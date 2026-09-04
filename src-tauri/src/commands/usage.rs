//! AI 模型用量统计与日志(ai_usage_log 表)。
//!
//! * 内置调用、Agent Harness、ACP agent 与定时报告均在 Rust 侧复用 `insert_usage_row`
//! * token 列可空:provider 未返回 usage 时行仍在,但不计入 SUM 汇总

use rusqlite::{params, Connection, Row};
use tauri::State;

use crate::db::Db;
use crate::error::AppResult;
use crate::models::{AiUsageDayStat, AiUsageEntry, AiUsageRecord, AiUsageSummary, AiUsageTaskStat};
use crate::time_util::now_ts;

/// 日志保留期:启动 prune 时清理更早的记录。
/// 190 天 ≈ 半年热力图窗口(27 周 = 189 天,含周对齐余量)
const RETENTION_SECS: i64 = 190 * 24 * 60 * 60;

/// 按日聚合返回的最大天数(前端热力图 27 周 = 189 天)
const BY_DAY_LIMIT: i64 = 190;

/// 明细日志分页单页大小上限(前端「加载更多」按此步进)
pub const LIST_PAGE_SIZE: i64 = 50;

/// 以固定 o200k_base 编码器统计文本 token。
///
/// 用于不绑定某个模型的本地文本统计，例如项目 AI 资产；tokenizer 固定可避免用户切换
/// 默认模型或聊天模型后，同一文件在界面上的计数发生变化。
pub(crate) fn count_o200k_tokens(text: &str) -> i64 {
    i64::try_from(tiktoken_rs::o200k_base_singleton().count_ordinary(text)).unwrap_or(i64::MAX)
}

/// ACP agent 未上报 usage 时的本地估算。优先按已知 OpenAI 模型选择编码器，
/// 未知/第三方模型统一回退 o200k_base；只覆盖应用可见的 prompt 与最终正文，
/// 不包含 agent 内部工具调用和上下文，因此结果是保守估算值。
pub(crate) fn estimate_text_tokens(model: &str, text: &str) -> i64 {
    let configured_model = model
        .rsplit_once(" · ")
        .map_or(model, |(_, configured)| configured);
    tiktoken_rs::bpe_for_model(configured_model)
        .map(|bpe| i64::try_from(bpe.count_ordinary(text)).unwrap_or(i64::MAX))
        .unwrap_or_else(|_| count_o200k_tokens(text))
}

pub(crate) fn insert_usage_row(
    conn: &Connection,
    record: &AiUsageRecord,
    created_at: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO ai_usage_log
             (created_at, task_type, model, input_tokens, output_tokens, total_tokens, duration_ms, cached_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            created_at,
            record.task_type,
            record.model,
            record.input_tokens,
            record.output_tokens,
            record.total_tokens,
            record.duration_ms,
            record.cached_tokens,
        ],
    )?;
    Ok(())
}

fn row_to_entry(r: &Row) -> rusqlite::Result<AiUsageEntry> {
    Ok(AiUsageEntry {
        id: r.get(0)?,
        created_at: r.get(1)?,
        task_type: r.get(2)?,
        model: r.get(3)?,
        input_tokens: r.get(4)?,
        output_tokens: r.get(5)?,
        total_tokens: r.get(6)?,
        duration_ms: r.get(7)?,
        cached_tokens: r.get(8)?,
    })
}

const ENTRY_COLS: &str = "id, created_at, task_type, model, input_tokens, output_tokens, total_tokens, duration_ms, cached_tokens";

/// 汇总统计:总计 + 按任务类型 + 最近 190 天按日(SUM 忽略 NULL)
#[tauri::command]
pub fn get_ai_usage_summary(db: State<Db>) -> AppResult<AiUsageSummary> {
    usage_summary(&db.0.lock().unwrap())
}

fn usage_summary(conn: &Connection) -> AppResult<AiUsageSummary> {
    let (total_calls, ti, to, tt, tc): (i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(cached_tokens), 0)
         FROM ai_usage_log",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;

    let mut by_task = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT task_type, COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(cached_tokens), 0)
         FROM ai_usage_log
         GROUP BY task_type
         ORDER BY COUNT(*) DESC, task_type",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(AiUsageTaskStat {
            task_type: r.get(0)?,
            calls: r.get(1)?,
            input_tokens: r.get(2)?,
            output_tokens: r.get(3)?,
            total_tokens: r.get(4)?,
            cached_tokens: r.get(5)?,
        })
    })?;
    for row in rows {
        by_task.push(row?);
    }

    // 按日本机时区分组(SQLite localtime 与 chrono Local 同源系统时区),取最近 BY_DAY_LIMIT 天倒序
    let mut by_day = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT date(created_at, 'unixepoch', 'localtime') AS day, COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(cached_tokens), 0)
         FROM ai_usage_log
         GROUP BY day
         ORDER BY day DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([BY_DAY_LIMIT], |r| {
        Ok(AiUsageDayStat {
            day: r.get(0)?,
            calls: r.get(1)?,
            input_tokens: r.get(2)?,
            output_tokens: r.get(3)?,
            total_tokens: r.get(4)?,
            cached_tokens: r.get(5)?,
        })
    })?;
    for row in rows {
        by_day.push(row?);
    }

    Ok(AiUsageSummary {
        total_calls,
        total_input_tokens: ti,
        total_output_tokens: to,
        total_tokens: tt,
        total_cached_tokens: tc,
        by_task,
        by_day,
    })
}

/// 明细日志倒序分页;task_type 过滤(None=全部)
#[tauri::command]
pub fn list_ai_usage_log(
    db: State<Db>,
    offset: i64,
    limit: i64,
    task_type: Option<String>,
) -> AppResult<Vec<AiUsageEntry>> {
    list_usage_rows(&db.0.lock().unwrap(), offset, limit, task_type.as_deref())
}

fn list_usage_rows(
    conn: &Connection,
    offset: i64,
    limit: i64,
    task_type: Option<&str>,
) -> AppResult<Vec<AiUsageEntry>> {
    let limit = limit.clamp(1, LIST_PAGE_SIZE);
    let sql = match task_type {
        Some(_) => format!(
            "SELECT {ENTRY_COLS} FROM ai_usage_log WHERE task_type = ?1
             ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        ),
        None => {
            format!("SELECT {ENTRY_COLS} FROM ai_usage_log ORDER BY id DESC LIMIT ?1 OFFSET ?2")
        }
    };
    let mut stmt = conn.prepare(&sql)?;
    let mapped = match task_type {
        Some(t) => stmt.query_map(params![t, limit, offset], row_to_entry)?,
        None => stmt.query_map(params![limit, offset], row_to_entry)?,
    };
    Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[tauri::command]
pub fn clear_ai_usage_log(db: State<Db>) -> AppResult<u32> {
    clear_usage_rows(&db.0.lock().unwrap())
}

/// 返回删除的行数(前端 toast 展示)
fn clear_usage_rows(conn: &Connection) -> AppResult<u32> {
    let deleted = conn.execute("DELETE FROM ai_usage_log", [])? as u32;
    Ok(deleted)
}

/// 启动清理超过保留期的记录(lib.rs setup 调用);失败仅记日志不阻断启动
pub(crate) fn prune_old_entries(conn: &Connection) {
    let cutoff = now_ts() - RETENTION_SECS;
    match conn.execute("DELETE FROM ai_usage_log WHERE created_at < ?1", [cutoff]) {
        Ok(n) if n > 0 => eprintln!("[usage] pruned {n} expired usage rows"),
        Ok(_) => {}
        Err(e) => eprintln!("[usage] prune failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use chrono::{Datelike, Local, TimeZone};

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    #[test]
    fn estimates_tokens_with_model_and_unknown_fallback() {
        assert_eq!(estimate_text_tokens("gpt-4o", "hello world"), 2);
        assert_eq!(
            estimate_text_tokens("Pi ACP adapter · company/custom-model", "hello world"),
            2,
        );
        assert_eq!(estimate_text_tokens("unknown", ""), 0);
    }

    #[test]
    fn counts_assets_with_fixed_o200k_tokenizer() {
        assert_eq!(count_o200k_tokens("hello world"), 2);
        assert_eq!(count_o200k_tokens(""), 0);
    }

    fn rec_cached(
        task: &str,
        input: Option<i64>,
        output: Option<i64>,
        total: Option<i64>,
        cached: Option<i64>,
    ) -> AiUsageRecord {
        AiUsageRecord {
            task_type: task.into(),
            model: "test-model".into(),
            input_tokens: input,
            output_tokens: output,
            total_tokens: total,
            duration_ms: Some(100),
            cached_tokens: cached,
        }
    }

    fn rec(
        task: &str,
        input: Option<i64>,
        output: Option<i64>,
        total: Option<i64>,
    ) -> AiUsageRecord {
        rec_cached(task, input, output, total, None)
    }

    /// 由 Unix 秒推导本机时区的 YYYY-MM-DD(SQLite date(...,'localtime') 的期望值)
    fn local_day(ts: i64) -> String {
        let d = Local.timestamp_opt(ts, 0).unwrap();
        format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
    }

    #[test]
    fn insert_and_summary_aggregate() {
        let conn = test_conn();
        let now = now_ts();
        insert_usage_row(
            &conn,
            &rec_cached("wiki", Some(100), Some(50), Some(150), Some(60)),
            now,
        )
        .unwrap();
        insert_usage_row(
            &conn,
            &rec_cached("wiki", Some(10), Some(5), Some(15), Some(4)),
            now,
        )
        .unwrap();
        insert_usage_row(&conn, &rec("commit", Some(200), Some(20), Some(220)), now).unwrap();
        // provider 未返回 usage 的行:计入次数,不计入求和
        insert_usage_row(&conn, &rec("report", None, None, None), now).unwrap();

        let s = usage_summary(&conn).unwrap();
        assert_eq!(s.total_calls, 4);
        assert_eq!(s.total_input_tokens, 310);
        assert_eq!(s.total_output_tokens, 75);
        assert_eq!(s.total_tokens, 385);
        assert_eq!(s.total_cached_tokens, 64);

        let wiki = s.by_task.iter().find(|t| t.task_type == "wiki").unwrap();
        assert_eq!(wiki.calls, 2);
        assert_eq!(wiki.total_tokens, 165);
        assert_eq!(wiki.cached_tokens, 64);
        // 无记录的任务类型不出现在分布里
        assert!(s.by_task.iter().all(|t| t.task_type != "nonexistent"));

        let today = local_day(now);
        let day = s.by_day.iter().find(|d| d.day == today).unwrap();
        assert_eq!(day.calls, 4);
        assert_eq!(day.total_tokens, 385);
        assert_eq!(day.cached_tokens, 64);

        // 明细行的缓存值可读回(id 倒序,排序后比较)
        let entries = list_usage_rows(&conn, 0, 50, None).unwrap();
        let mut cached_vals: Vec<_> = entries
            .iter()
            .filter(|e| e.task_type == "wiki")
            .map(|e| e.cached_tokens)
            .collect();
        cached_vals.sort();
        assert_eq!(cached_vals, vec![Some(4), Some(60)]);
        let commit_entry = entries.iter().find(|e| e.task_type == "commit").unwrap();
        assert_eq!(commit_entry.cached_tokens, None);
    }

    #[test]
    fn by_day_groups_by_local_date() {
        let conn = test_conn();
        // 相隔 26 小时的两个时间戳:即使跨夏令时也不会落在同一本地日
        let base = now_ts() - 26 * 3600;
        insert_usage_row(&conn, &rec("commit", Some(1), Some(1), Some(2)), base).unwrap();
        insert_usage_row(&conn, &rec("commit", Some(3), Some(3), Some(6)), now_ts()).unwrap();

        let s = usage_summary(&conn).unwrap();
        assert_eq!(s.by_day.len(), 2);
        assert_eq!(s.by_day[0].day, local_day(now_ts()));
        assert_eq!(s.by_day[1].day, local_day(base));
        assert!(s.by_day[0].day > s.by_day[1].day, "按日应倒序");
    }

    #[test]
    fn list_filters_and_paginates() {
        let conn = test_conn();
        for i in 0..7 {
            let task = if i % 2 == 0 { "commit" } else { "wiki" };
            insert_usage_row(&conn, &rec(task, Some(i), None, Some(i)), now_ts()).unwrap();
        }
        // id 倒序
        let all = list_usage_rows(&conn, 0, 50, None).unwrap();
        assert_eq!(all.len(), 7);
        assert_eq!(all[0].total_tokens, Some(6));
        assert_eq!(all[0].model, "test-model");
        // 筛选
        let commits = list_usage_rows(&conn, 0, 50, Some("commit")).unwrap();
        assert_eq!(commits.len(), 4);
        assert!(commits.iter().all(|e| e.task_type == "commit"));
        // 分页:offset 跳过、越界为空
        let page = list_usage_rows(&conn, 6, 50, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].total_tokens, Some(0));
        assert_eq!(list_usage_rows(&conn, 100, 50, None).unwrap().len(), 0);
        // limit 钳制到单页上限
        for _ in 7..LIST_PAGE_SIZE + 3 {
            insert_usage_row(&conn, &rec("commit", None, None, None), now_ts()).unwrap();
        }
        assert_eq!(
            list_usage_rows(&conn, 0, 10_000, None).unwrap().len() as i64,
            LIST_PAGE_SIZE
        );
    }

    #[test]
    fn clear_removes_all() {
        let conn = test_conn();
        insert_usage_row(&conn, &rec("commit", Some(1), Some(1), Some(2)), now_ts()).unwrap();
        assert_eq!(clear_usage_rows(&conn).unwrap(), 1);
        assert_eq!(usage_summary(&conn).unwrap().total_calls, 0);
        // 幂等
        assert_eq!(clear_usage_rows(&conn).unwrap(), 0);
    }

    #[test]
    fn prune_only_deletes_expired() {
        let conn = test_conn();
        let old = now_ts() - RETENTION_SECS - 3600;
        let fresh = now_ts() - 60;
        insert_usage_row(&conn, &rec("commit", None, None, None), old).unwrap();
        insert_usage_row(&conn, &rec("commit", Some(1), Some(1), Some(2)), fresh).unwrap();
        prune_old_entries(&conn);
        let left = list_usage_rows(&conn, 0, 50, None).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].created_at, fresh);
    }
}
