//! 报告历史记录、详情查询与日历聚合。

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::db::Db;
use crate::error::AppResult;
use crate::models::GitCommitInfo;

// ── types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHistoryItem {
    pub id: i64,
    pub project_ids: Vec<i64>,
    pub date_from: String,
    pub date_to: String,
    pub range_label: String,
    pub author_mode: String,
    pub language: String,
    /// "daily" | "weekly"
    pub period_type: String,
    pub created_at: i64,
    pub project_names: Vec<String>,
    pub total_commits: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHistoryDetail {
    #[serde(flatten)]
    pub item: ReportHistoryItem,
    pub result: String,
    pub commits: Vec<ReportCommitItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportCommitItem {
    pub project_id: Option<i64>,
    pub project_name: String,
    pub project_description: String,
    pub commits: Vec<GitCommitInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReportCommit {
    pub(crate) project_id: Option<i64>,
    pub(crate) project_name: String,
    #[serde(default)]
    pub(crate) project_description: String,
    pub(crate) commits: Vec<GitCommitInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportGeneratedPayload {
    pub schedule_name: String,
    pub history_id: i64,
    pub date_from: String,
    pub date_to: String,
}

// ── commands: report history ───────────────────────────────────────────

/// 写入报告历史的内部入口。手动、批量与 AI 后端生成共用这一实现，确保报告正文、
/// 提交快照和刷新事件始终由后端一次性完成。
/// `app` 为 None 时(MCP 等 headless 场景)只落库,不发前端事件。
pub(crate) fn save_report_history_impl(
    app: Option<&AppHandle>,
    conn: &Connection,
    project_ids: &[i64],
    date_from: &str,
    date_to: &str,
    range_label: &str,
    author_mode: &str,
    language: &str,
    period_type: &str,
    result: &str,
    commit_data: &[SaveReportCommit],
) -> AppResult<i64> {
    let now = crate::time_util::now_ts();
    let ids_json = serde_json::to_string(&project_ids).unwrap_or_default();

    conn.execute(
        "INSERT INTO report_history (project_ids, date_from, date_to, range_label, author_mode, language, period_type, result, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![ids_json, date_from, date_to, range_label, author_mode, language, period_type, result, now],
    )?;
    let report_id = conn.last_insert_rowid();

    for item in commit_data {
        let commits_json = serde_json::to_string(&item.commits).unwrap_or_default();
        conn.execute(
            "INSERT INTO report_commits (report_id, project_id, project_name, project_description, commit_data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                report_id,
                item.project_id,
                item.project_name,
                item.project_description,
                commits_json,
            ],
        )?;
    }

    // 通知前端刷新(报告历史页日历/列表);与 scheduler 定时生成共用同一事件,
    // 手动/批量生成没有任务名,schedule_name 置空
    if let Some(app) = app {
        let payload = ReportGeneratedPayload {
            schedule_name: String::new(),
            history_id: report_id,
            date_from: date_from.to_string(),
            date_to: date_to.to_string(),
        };
        if let Err(e) = app.emit("report://generated", payload) {
            eprintln!("[report] 发送前端通知失败: {e}");
        }
    }

    Ok(report_id)
}

/// 分页查询报告历史列表,可按项目筛选。
pub fn list_report_history(
    db: State<'_, Db>,
    limit: Option<usize>,
    offset: Option<usize>,
    project_id: Option<i64>,
) -> AppResult<Vec<ReportHistoryItem>> {
    let conn = db.0.lock().unwrap();
    list_report_history_impl(&conn, limit, offset, project_id)
}

/// 分页查询报告历史列表的实现(可在测试中传入内存连接)。
///
/// `project_id` 使用 `json_each` 在 `project_ids` JSON 数组中做精确元素匹配,
/// 避免 LIKE '%pid%' 模糊匹配导致 1/12/123 互相命中。
pub fn list_report_history_impl(
    conn: &Connection,
    limit: Option<usize>,
    offset: Option<usize>,
    project_id: Option<i64>,
) -> AppResult<Vec<ReportHistoryItem>> {
    let limit = limit.unwrap_or(50).min(200);
    let offset = offset.unwrap_or(0);

    let rows = if let Some(pid) = project_id {
        let mut stmt = conn.prepare(
            "SELECT h.id, h.project_ids, h.date_from, h.date_to, h.range_label,
                    h.author_mode, h.language, h.period_type, h.created_at
             FROM report_history h
             WHERE EXISTS (
                 SELECT 1 FROM json_each(h.project_ids) WHERE CAST(value AS INTEGER) = ?1
             )
             ORDER BY h.created_at DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let collected = stmt
            .query_map(params![pid, limit as i64, offset as i64], map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        collected
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, project_ids, date_from, date_to, range_label,
                    author_mode, language, period_type, created_at
             FROM report_history
             ORDER BY created_at DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let collected = stmt
            .query_map(params![limit as i64, offset as i64], map_row)?
            .collect::<Result<Vec<_>, _>>()?;
        collected
    };

    // 一次查询所有项目的名称与提交数,避免 N+1
    let (report_ids, project_id_set) = collect_report_pairs(&rows);
    let name_map = if project_id_set.is_empty() {
        HashMap::new()
    } else {
        resolve_project_names_batch(conn, &project_id_set)?
    };
    let count_map = if report_ids.is_empty() {
        HashMap::new()
    } else {
        count_commits_batch(conn, &report_ids)?
    };

    let mut items = Vec::with_capacity(rows.len());
    for (mut item, ids) in rows {
        let mut names: Vec<String> = ids
            .iter()
            .filter_map(|id| name_map.get(id).cloned())
            .collect();
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        item.project_names = names;
        item.total_commits = count_map.get(&item.id).copied().unwrap_or(0);
        items.push(item);
    }

    Ok(items)
}

/// 查询单条报告详情(含 Markdown 正文与提交记录)。
pub fn get_report_history(db: State<'_, Db>, id: i64) -> AppResult<ReportHistoryDetail> {
    let conn = db.0.lock().unwrap();

    let (mut item, ids, result) = conn.query_row(
        "SELECT id, project_ids, date_from, date_to, range_label,
                author_mode, language, period_type, created_at, result
         FROM report_history WHERE id = ?1",
        params![id],
        |r| {
            let ids_json: String = r.get(1)?;
            let ids: Vec<i64> = serde_json::from_str(&ids_json).unwrap_or_default();
            Ok((
                ReportHistoryItem {
                    id: r.get(0)?,
                    project_ids: ids.clone(),
                    date_from: r.get(2)?,
                    date_to: r.get(3)?,
                    range_label: r.get(4)?,
                    author_mode: r.get(5)?,
                    language: r.get(6)?,
                    period_type: r.get(7)?,
                    created_at: r.get(8)?,
                    project_names: Vec::new(),
                    total_commits: 0,
                },
                ids,
                r.get::<_, String>(9)?,
            ))
        },
    )?;

    item.project_names = resolve_project_names(&conn, &ids)?;
    item.total_commits = count_commits(&conn, item.id)?;

    let commits = load_report_commits(&conn, item.id)?;

    Ok(ReportHistoryDetail {
        item,
        result,
        commits,
    })
}

/// 删除报告历史(级联删除关联的提交记录)。
pub fn delete_report_history(db: State<'_, Db>, id: i64) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    delete_report_history_impl(&conn, id)
}

/// 删除报告历史的纯实现(供 scheduler 的孤儿清理路径与测试复用)。
/// `report_commits` 通过外键级联删除。
pub fn delete_report_history_impl(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM report_history WHERE id = ?1", params![id])?;
    Ok(())
}

// ── helpers ────────────────────────────────────────────────────────────

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(ReportHistoryItem, Vec<i64>)> {
    let ids_json: String = r.get(1)?;
    let ids: Vec<i64> = serde_json::from_str(&ids_json).unwrap_or_default();
    Ok((
        ReportHistoryItem {
            id: r.get(0)?,
            project_ids: ids.clone(),
            date_from: r.get(2)?,
            date_to: r.get(3)?,
            range_label: r.get(4)?,
            author_mode: r.get(5)?,
            language: r.get(6)?,
            period_type: r.get(7)?,
            created_at: r.get(8)?,
            project_names: Vec::new(),
            total_commits: 0,
        },
        ids,
    ))
}

/// 映射 get_reports_by_range 的查询行(末尾多一列 result)
pub(super) fn map_detail_row(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<(ReportHistoryItem, Vec<i64>, String)> {
    let (item, ids) = map_row(r)?;
    Ok((item, ids, r.get::<_, String>(9)?))
}

fn resolve_project_names(conn: &Connection, ids: &[i64]) -> AppResult<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let sql = format!(
        "SELECT name FROM projects WHERE id IN ({}) ORDER BY name COLLATE NOCASE",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();
    let names = stmt
        .query_map(params.as_slice(), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

fn count_commits(conn: &Connection, report_id: i64) -> AppResult<i64> {
    let count: i64 = conn.query_row(
        "SELECT COALESCE(SUM(json_array_length(commit_data)), 0) FROM report_commits WHERE report_id = ?1",
        params![report_id],
        |r| r.get(0),
    )?;
    Ok(count)
}

fn load_report_commits(conn: &Connection, report_id: i64) -> AppResult<Vec<ReportCommitItem>> {
    let mut stmt = conn.prepare(
        "SELECT project_id, project_name, project_description, commit_data
         FROM report_commits WHERE report_id = ?1",
    )?;
    let commits = stmt
        .query_map(params![report_id], |r| {
            let data_json: String = r.get(3)?;
            let commits: Vec<GitCommitInfo> = serde_json::from_str(&data_json).unwrap_or_default();
            Ok(ReportCommitItem {
                project_id: r.get(0)?,
                project_name: r.get(1)?,
                project_description: r.get(2)?,
                commits,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(commits)
}

// ── batch helpers ─────────────────────────────────────────────────────

/// 从一批 `(ReportHistoryItem, Vec<i64>)` 行中收集所有 report id 与关联 project id。
/// 用于单次批量查询 `project_names`、`total_commits`、`commits`。
fn collect_report_pairs(rows: &[(ReportHistoryItem, Vec<i64>)]) -> (Vec<i64>, Vec<i64>) {
    let mut report_ids: Vec<i64> = Vec::with_capacity(rows.len());
    let mut project_set: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (item, ids) in rows {
        report_ids.push(item.id);
        for pid in ids {
            project_set.insert(*pid);
        }
    }
    let project_ids: Vec<i64> = project_set.into_iter().collect();
    (report_ids, project_ids)
}

/// 从一批 `(ReportHistoryItem, Vec<i64>, String)` 行中收集所有 report id 与关联 project id。
/// `String` 是附加的 `result` 列,这里忽略。
pub(super) fn collect_report_triples(
    rows: &[(ReportHistoryItem, Vec<i64>, String)],
) -> (Vec<i64>, Vec<i64>) {
    let mut report_ids: Vec<i64> = Vec::with_capacity(rows.len());
    let mut project_set: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (item, ids, _result) in rows {
        report_ids.push(item.id);
        for pid in ids {
            project_set.insert(*pid);
        }
    }
    let project_ids: Vec<i64> = project_set.into_iter().collect();
    (report_ids, project_ids)
}

/// 一次性批量加载多个项目的名称(避免 N+1)。空 ids 直接返回空 map。
pub fn resolve_project_names_batch(
    conn: &Connection,
    ids: &[i64],
) -> AppResult<HashMap<i64, String>> {
    let mut map = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT id, name FROM projects WHERE id IN ({})",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();
    let rows = stmt.query_map(params.as_slice(), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, name) = row?;
        map.insert(id, name);
    }
    Ok(map)
}

/// 一次性批量统计多个报告的提交总数(避免 N+1)。空 ids 直接返回空 map。
pub fn count_commits_batch(conn: &Connection, report_ids: &[i64]) -> AppResult<HashMap<i64, i64>> {
    let mut map = HashMap::new();
    if report_ids.is_empty() {
        return Ok(map);
    }
    let placeholders: Vec<String> = (0..report_ids.len())
        .map(|i| format!("?{}", i + 1))
        .collect();
    let sql = format!(
        "SELECT report_id, COALESCE(SUM(json_array_length(commit_data)), 0)
         FROM report_commits WHERE report_id IN ({}) GROUP BY report_id",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = report_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();
    let rows = stmt.query_map(params.as_slice(), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (rid, cnt) = row?;
        map.insert(rid, cnt);
    }
    Ok(map)
}

/// 一次性批量加载多个报告的提交明细(避免 N+1)。空 ids 直接返回空 map。
pub fn load_report_commits_batch(
    conn: &Connection,
    report_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<ReportCommitItem>>> {
    let mut map: HashMap<i64, Vec<ReportCommitItem>> = HashMap::new();
    if report_ids.is_empty() {
        return Ok(map);
    }
    let placeholders: Vec<String> = (0..report_ids.len())
        .map(|i| format!("?{}", i + 1))
        .collect();
    let sql = format!(
        "SELECT report_id, project_id, project_name, project_description, commit_data
         FROM report_commits WHERE report_id IN ({}) ORDER BY report_id, id",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = report_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();
    let rows = stmt.query_map(params.as_slice(), |r| {
        let rid: i64 = r.get(0)?;
        let data_json: String = r.get(4)?;
        let commits: Vec<GitCommitInfo> = serde_json::from_str(&data_json).unwrap_or_default();
        Ok((
            rid,
            ReportCommitItem {
                project_id: r.get(1)?,
                project_name: r.get(2)?,
                project_description: r.get(3)?,
                commits,
            },
        ))
    })?;
    for row in rows {
        let (rid, item) = row?;
        map.entry(rid).or_default().push(item);
    }
    Ok(map)
}
