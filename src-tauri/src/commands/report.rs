//! 日报/周报历史与定时任务管理。
//!
//! * 报告历史存 SQLite(report_history + report_commits 表)
//! * 定时任务配置存 SQLite(report_schedules 表)
//! * 定时任务变更时通过 Notify 唤醒后台 scheduler

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::GitCommitInfo;
use crate::workday;

// ── types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSchedule {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// "daily" | "weekly"
    #[serde(default = "default_report_type")]
    pub report_type: String,
    pub project_ids: Vec<i64>,
    #[serde(default = "default_author_mode")]
    pub author_mode: String,
    pub time_of_day: String,
    /// 日报:仅周一~周五
    #[serde(default)]
    pub weekdays_only: bool,
    /// 日报:仅中国工作日
    #[serde(default)]
    pub chinese_workday_only: bool,
    /// 周报:true = 工作周模式(自动识别连续工作周期,末日触发);
    /// false = 自定义周几~周几(结束周几触发)
    #[serde(default = "default_weekly_workweek")]
    pub weekly_workweek: bool,
    /// 周报自定义:范围起始周几(1=周一 .. 7=周日)
    #[serde(default = "default_weekly_start")]
    pub weekly_start_weekday: u32,
    /// 周报自定义:范围结束/触发周几(1=周一 .. 7=周日)
    #[serde(default = "default_weekly_end")]
    pub weekly_end_weekday: u32,
    #[serde(default)]
    pub last_run_at: Option<i64>,
}

fn default_enabled() -> bool {
    true
}
fn default_author_mode() -> String {
    "me".into()
}
fn default_report_type() -> String {
    "daily".into()
}
fn default_weekly_workweek() -> bool {
    true
}
fn default_weekly_start() -> u32 {
    1
}
fn default_weekly_end() -> u32 {
    5
}

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
    pub project_id: Option<i64>,
    pub project_name: String,
    #[serde(default)]
    pub project_description: String,
    pub commits: Vec<GitCommitInfo>,
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

/// 保存报告(日报/周报)及其提交记录到历史,返回新记录 id。
/// 前端在生成报告后自动调用此命令(无需手动操作)。
#[tauri::command]
pub fn save_report_history(
    app: AppHandle,
    db: State<'_, Db>,
    project_ids: Vec<i64>,
    date_from: String,
    date_to: String,
    range_label: String,
    author_mode: String,
    language: String,
    period_type: String,
    result: String,
    commit_data: Vec<SaveReportCommit>,
) -> AppResult<i64> {
    let conn = db.0.lock().unwrap();
    let now = chrono::Utc::now().timestamp();
    let ids_json = serde_json::to_string(&project_ids).unwrap_or_default();

    conn.execute(
        "INSERT INTO report_history (project_ids, date_from, date_to, range_label, author_mode, language, period_type, result, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![ids_json, date_from, date_to, range_label, author_mode, language, period_type, result, now],
    )?;
    let report_id = conn.last_insert_rowid();

    for item in &commit_data {
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
    let payload = ReportGeneratedPayload {
        schedule_name: String::new(),
        history_id: report_id,
        date_from,
        date_to,
    };
    if let Err(e) = app.emit("report://generated", payload) {
        eprintln!("[report] 发送前端通知失败: {e}");
    }

    Ok(report_id)
}

/// 分页查询报告历史列表,可按项目筛选。
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
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

// ── calendar meta ──────────────────────────────────────────────────────

/// 日历标注数据：某月每天的报告数量 + 节假日/调休列表。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarMeta {
    pub dates: HashMap<String, i64>,
    pub holidays: Vec<String>,
    pub workdays: Vec<String>,
}

/// 为动态 WHERE 条件追加项目/标签/类型过滤(供日历与按日查询共用)。
/// `conditions` 中已包含日期条件(`?1` 起),本函数从 `param_idx` 继续编号。
fn append_history_filters(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    param_idx: &mut usize,
    project_ids: &[i64],
    tag_ids: &[i64],
    report_type: &Option<String>,
) {
    if !project_ids.is_empty() {
        let placeholders: Vec<String> = (0..project_ids.len())
            .map(|i| format!("?{}", *param_idx + i))
            .collect();
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM json_each(h.project_ids) WHERE CAST(value AS INTEGER) IN ({}))",
            placeholders.join(",")
        ));
        for pid in project_ids {
            params_vec.push(Box::new(*pid));
        }
        *param_idx += project_ids.len();
    }

    if !tag_ids.is_empty() {
        let placeholders: Vec<String> = (0..tag_ids.len())
            .map(|i| format!("?{}", *param_idx + i))
            .collect();
        let having_param = format!("?{}", *param_idx + tag_ids.len());
        conditions.push(format!(
            "EXISTS (SELECT 1 FROM json_each(h.project_ids) j \
             WHERE CAST(j.value AS INTEGER) IN ( \
                 SELECT pt.project_id FROM project_tags pt \
                 WHERE pt.tag_id IN ({}) \
                 GROUP BY pt.project_id \
                 HAVING COUNT(DISTINCT pt.tag_id) = {} \
             ))",
            placeholders.join(","),
            having_param,
        ));
        for tid in tag_ids {
            params_vec.push(Box::new(*tid));
        }
        params_vec.push(Box::new(tag_ids.len() as i64));
        *param_idx += tag_ids.len() + 1;
    }

    if let Some(rt) = report_type {
        conditions.push(format!("h.period_type = ?{param_idx}"));
        params_vec.push(Box::new(rt.clone()));
    }
}

/// 返回某月的日历标注数据(每天报告数 + 节假日/调休),供前端日历渲染。
/// 日期按报告范围的最后一天(date_to)标记:日报为所选日期,周报为范围末日。
#[tauri::command]
pub fn get_calendar_meta(
    db: State<'_, Db>,
    year: i32,
    month: u32,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<CalendarMeta> {
    let conn = db.0.lock().unwrap();
    let dates = get_calendar_meta_impl(&conn, year, month, &project_ids, &tag_ids, &report_type)?;

    // 节假日/调休数据
    let (holidays, workdays) = workday::load_data(&workday::cache_root()).unwrap_or_default();
    let holidays: Vec<String> = holidays.into_iter().collect();
    let workdays: Vec<String> = workdays.into_iter().collect();

    Ok(CalendarMeta {
        dates,
        holidays,
        workdays,
    })
}

/// 日历聚合实现:按月用一次 GROUP BY 查询各 `date_to` 的报告计数。
///
/// 区间对齐到 reka-ui `CalendarRoot` 实际渲染的月视图网格(周一开始、自适应周数,
/// 最多 6 行 × 7 列 = 42 格),而非仅当月自然范围——这样上月末尾与下月开头的填充日
/// 也能拿到报告计数,前端 `getReportCount` 即可在那些格子上显示标注。
pub fn get_calendar_meta_impl(
    conn: &Connection,
    year: i32,
    month: u32,
    project_ids: &[i64],
    tag_ids: &[i64],
    report_type: &Option<String>,
) -> AppResult<HashMap<String, i64>> {
    let month_start = NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| AppError::coded(ErrorCode::ReportInvalidYearMonth, format!("year={year} month={month}")))?;
    // 网格首格 = 当月 1 日向前对齐到所在周的周一(num_days_from_monday() ∈ 0..6)
    let grid_start = month_start
        - chrono::Duration::days(month_start.weekday().num_days_from_monday() as i64);
    // 网格末格 = 首格 + 41 天(周一 + 41 天 = 周日,覆盖 6 行)
    let grid_end = grid_start + chrono::Duration::days(41);
    let date_from = grid_start.format("%Y-%m-%d").to_string();
    let date_to_inclusive = grid_end.format("%Y-%m-%d").to_string();

    let mut conditions = vec!["h.date_to BETWEEN ?1 AND ?2".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
        vec![Box::new(date_from), Box::new(date_to_inclusive)];
    let mut param_idx = 3;
    append_history_filters(
        &mut conditions,
        &mut params_vec,
        &mut param_idx,
        project_ids,
        tag_ids,
        report_type,
    );

    let sql = format!(
        "SELECT h.date_to, COUNT(*) FROM report_history h WHERE {} GROUP BY h.date_to",
        conditions.join(" AND ")
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;

    let mut dates: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (d, c) = row?;
        dates.insert(d, c);
    }
    Ok(dates)
}

/// 节假日/调休标注数据(全集),供报告生成弹窗等日期选择日历做高亮。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HolidayData {
    pub holidays: Vec<String>,
    pub workdays: Vec<String>,
}

/// 返回全部法定节假日/调休补班日期列表(数据源覆盖 2004–2026,体积小,一次返回全集)。
/// 数据加载失败时回退空集合,前端日历退化为常规周末着色。
#[tauri::command]
pub fn get_holiday_data() -> AppResult<HolidayData> {
    let (holidays, workdays) = workday::load_data(&workday::cache_root()).unwrap_or_default();
    Ok(HolidayData {
        holidays: holidays.into_iter().collect(),
        workdays: workdays.into_iter().collect(),
    })
}

/// 查询指定日期(date_to 匹配)的所有报告详情(含提交记录和 Markdown 正文)。
#[tauri::command]
pub fn get_reports_by_date(
    db: State<'_, Db>,
    date: String,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    let conn = db.0.lock().unwrap();
    get_reports_by_date_impl(&conn, &date, &project_ids, &tag_ids, &report_type)
}

/// 按日期查询的实现:一次加载所有报告的 project_names/total_commits/commits。
pub fn get_reports_by_date_impl(
    conn: &Connection,
    date: &str,
    project_ids: &[i64],
    tag_ids: &[i64],
    report_type: &Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    let mut conditions = vec!["h.date_to = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(date.to_string())];
    let mut param_idx = 2;
    append_history_filters(
        &mut conditions,
        &mut params_vec,
        &mut param_idx,
        project_ids,
        tag_ids,
        report_type,
    );

    let sql = format!(
        "SELECT id, project_ids, date_from, date_to, range_label,
                author_mode, language, period_type, created_at, result
         FROM report_history h
         WHERE {}
         ORDER BY h.created_at DESC",
        conditions.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params_vec), map_detail_row)?
        .collect::<Result<Vec<_>, _>>()?;

    // 一次加载全部 project_names、total_commits、commits,避免 N+1
    let (report_ids, project_id_set) = collect_report_triples(&rows);
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
    let commits_map = if report_ids.is_empty() {
        HashMap::new()
    } else {
        load_report_commits_batch(conn, &report_ids)?
    };

    let mut results = Vec::with_capacity(rows.len());
    for (mut item, ids, result) in rows {
        let mut names: Vec<String> = ids
            .iter()
            .filter_map(|id| name_map.get(id).cloned())
            .collect();
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        item.project_names = names;
        item.total_commits = count_map.get(&item.id).copied().unwrap_or(0);
        let commits = commits_map.get(&item.id).cloned().unwrap_or_default();

        results.push(ReportHistoryDetail {
            item,
            result,
            commits,
        });
    }

    Ok(results)
}

// ── commands: work week ranges ─────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkWeekRange {
    /// 起止日期 "YYYY-MM-DD"
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkWeekRanges {
    pub this_week: WorkWeekRange,
    pub last_week: WorkWeekRange,
}

/// 本周/上周工作周的具体日期范围,供生成报告弹窗展示与查询。
///
/// 复用 scheduler 的工作周算法(连续工作周期,法定节假日/调休按 chinese-days
/// 数据识别):本周 = 当前工作周起点 ~ 今天;上周 = 上一个完整工作周。
/// 同步 #[tauri::command] 在主线程跑,is_workday 可能读盘/拉 CDN,放入线程池。
#[tauri::command]
pub async fn get_work_week_ranges() -> AppResult<WorkWeekRanges> {
    tokio::task::spawn_blocking(move || {
        let cache_root = workday::cache_root();
        let today = chrono::Local::now().date_naive();
        let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();

        let this_start = crate::scheduler::work_week_start(today, &cache_root);
        // 上一个工作周:本周工作周起点之前最近的工作日即其末日
        // (14 天上限覆盖整周法定节假日的情况)
        let mut last_end = this_start - chrono::Duration::days(1);
        for _ in 0..14 {
            if workday::is_workday(last_end, &cache_root) {
                break;
            }
            last_end -= chrono::Duration::days(1);
        }
        let last_start = crate::scheduler::work_week_start(last_end, &cache_root);

        Ok(WorkWeekRanges {
            this_week: WorkWeekRange {
                from: fmt(this_start),
                to: fmt(today),
            },
            last_week: WorkWeekRange {
                from: fmt(last_start),
                to: fmt(last_end),
            },
        })
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::ReportTaskFailed, e.to_string()))?
}

// ── commands: batch report planning ────────────────────────────────────

/// 批量生成的单个时段(daily: 单日; weekly: 一个工作周)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRange {
    pub date_from: String,
    pub date_to: String,
    pub is_workday: bool,
}

/// 已有报告的日期范围(供批量生成"跳过已有"匹配)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDateRange {
    pub date_from: String,
    pub date_to: String,
}

/// 批量日报跨度上限(天)
const BATCH_DAILY_MAX_DAYS: i64 = 93;
/// 批量周报跨度上限(天)
const BATCH_WEEKLY_MAX_DAYS: i64 = 180;

fn parse_date(s: &str) -> AppResult<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::coded(ErrorCode::ReportInvalidDate, s.to_string()))
}

/// 规划批量生成的时段列表。
/// * daily: 枚举范围内每一天,is_workday 标注是否工作日(过滤模式由前端决定)
/// * weekly: 按工作周(连续工作周期)切段,复用 scheduler 的工作周算法
/// is_workday 可能读盘/拉 CDN,放入线程池执行。
#[tauri::command]
pub async fn plan_batch_report_ranges(
    period_type: String,
    date_from: String,
    date_to: String,
) -> AppResult<Vec<BatchRange>> {
    tokio::task::spawn_blocking(move || {
        let from = parse_date(&date_from)?;
        let to = parse_date(&date_to)?;
        if from > to {
            return Err(AppError::coded(ErrorCode::ReportDateRangeInverted, ""));
        }
        let span = (to - from).num_days();
        let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();

        let checker = workday::WorkdayChecker::load(&workday::cache_root());
        let is_workday = |d: NaiveDate| checker.is_workday(d);

        if period_type == "weekly" {
            if span > BATCH_WEEKLY_MAX_DAYS {
                return Err(AppError::coded(
                    ErrorCode::ReportBatchWeeklySpanExceeded,
                    BATCH_WEEKLY_MAX_DAYS.to_string(),
                ));
            }
            // 逐日扫描,工作日且为工作周末日时闭合一段;末尾不足一周的收尾
            let mut ranges = Vec::new();
            let mut seg_start = from;
            let mut d = from;
            while d <= to {
                if crate::scheduler::is_work_week_last_day_with(d, &is_workday) {
                    ranges.push(BatchRange {
                        date_from: fmt(seg_start),
                        date_to: fmt(d),
                        is_workday: true,
                    });
                    seg_start = d + chrono::Duration::days(1);
                }
                d += chrono::Duration::days(1);
            }
            if seg_start <= to {
                ranges.push(BatchRange {
                    date_from: fmt(seg_start),
                    date_to: fmt(to),
                    is_workday: true,
                });
            }
            Ok(ranges)
        } else {
            if span > BATCH_DAILY_MAX_DAYS {
                return Err(AppError::coded(
                    ErrorCode::ReportBatchDailySpanExceeded,
                    BATCH_DAILY_MAX_DAYS.to_string(),
                ));
            }
            let mut ranges = Vec::new();
            let mut d = from;
            while d <= to {
                ranges.push(BatchRange {
                    date_from: fmt(d),
                    date_to: fmt(d),
                    is_workday: is_workday(d),
                });
                d += chrono::Duration::days(1);
            }
            Ok(ranges)
        }
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::ReportTaskFailed, e.to_string()))?
}

/// 查询范围内已有报告的日期范围列表(按 period_type 过滤,去重)。
/// 前端匹配规则:日报按 date_to 相等跳过;周报按 (date_from, date_to) 对相等跳过。
#[tauri::command]
pub fn list_report_dates(
    db: State<'_, Db>,
    period_type: String,
    date_from: String,
    date_to: String,
) -> AppResult<Vec<ReportDateRange>> {
    let conn = db.0.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT date_from, date_to FROM report_history
         WHERE period_type = ?1 AND date_to BETWEEN ?2 AND ?3
         ORDER BY date_to",
    )?;
    let rows = stmt
        .query_map(params![period_type, date_from, date_to], |r| {
            Ok(ReportDateRange {
                date_from: r.get(0)?,
                date_to: r.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── commands: schedules ────────────────────────────────────────────────

const SCHEDULE_COLS: &str = "id, name, enabled, report_type, project_ids, author_mode, \
     time_of_day, weekdays_only, chinese_workday_only, weekly_workweek, weekly_start_weekday, \
     weekly_end_weekday, last_run_at";

fn map_schedule_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReportSchedule> {
    let ids_json: String = r.get(4)?;
    Ok(ReportSchedule {
        id: r.get(0)?,
        name: r.get(1)?,
        enabled: r.get::<_, i64>(2)? != 0,
        report_type: r.get(3)?,
        project_ids: serde_json::from_str(&ids_json).unwrap_or_default(),
        author_mode: r.get(5)?,
        time_of_day: r.get(6)?,
        weekdays_only: r.get::<_, i64>(7)? != 0,
        chinese_workday_only: r.get::<_, i64>(8)? != 0,
        weekly_workweek: r.get::<_, i64>(9)? != 0,
        weekly_start_weekday: r.get::<_, u32>(10)?,
        weekly_end_weekday: r.get::<_, u32>(11)?,
        last_run_at: r.get(12)?,
    })
}

/// 读取定时任务配置列表。
#[tauri::command]
pub fn list_report_schedules(db: State<'_, Db>) -> AppResult<Vec<ReportSchedule>> {
    let conn = db.0.lock().unwrap();
    read_schedules(&conn)
}

/// 保存定时任务配置(全量替换),同时唤醒后台 scheduler 重算下次触发时间。
#[tauri::command]
pub fn save_report_schedules(
    db: State<'_, Db>,
    app: AppHandle,
    schedules: Vec<ReportSchedule>,
) -> AppResult<()> {
    {
        let mut conn = db.0.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM report_schedules", [])?;
        for s in &schedules {
            let ids_json = serde_json::to_string(&s.project_ids).unwrap_or_default();
            tx.execute(
                "INSERT INTO report_schedules (id, name, enabled, report_type, project_ids, author_mode,
                     time_of_day, weekdays_only, chinese_workday_only, weekly_workweek,
                     weekly_start_weekday, weekly_end_weekday, last_run_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    s.id,
                    s.name,
                    s.enabled as i64,
                    s.report_type,
                    ids_json,
                    s.author_mode,
                    s.time_of_day,
                    s.weekdays_only as i64,
                    s.chinese_workday_only as i64,
                    s.weekly_workweek as i64,
                    s.weekly_start_weekday,
                    s.weekly_end_weekday,
                    s.last_run_at,
                ],
            )?;
        }
        tx.commit()?;
    }

    // 唤醒 scheduler(若尚未启动则忽略)
    if let Some(notify) = app.try_state::<ScheduleNotify>() {
        notify.0.notify_one();
    }
    Ok(())
}

/// 手动执行某个定时任务:忽略星期/去重检查,立即按任务配置生成报告。
/// 返回新报告历史 id;无提交记录或 AI 调用失败时返回错误文案。
#[tauri::command]
pub async fn run_report_schedule_now(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
) -> AppResult<i64> {
    let schedule = {
        let conn = db.0.lock().unwrap();
        let sql = format!("SELECT {SCHEDULE_COLS} FROM report_schedules WHERE id = ?1");
        conn.query_row(&sql, params![id], map_schedule_row)
            .map_err(|_| AppError::coded(ErrorCode::ScheduleNotFound, id.to_string()))?
    };
    let data_dir = workday::data_dir(&app);
    let client = reqwest::Client::new();
    crate::scheduler::fire_schedule(&app, &client, &data_dir, &schedule).await
}

// ── Notify wrapper for Tauri state ─────────────────────────────────────

/// 用于 Tauri 托管状态的 Notify 包装(Arc<Notify> 自身不满足 State 要求)
pub struct ScheduleNotify(pub Arc<Notify>);

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

/// 映射 get_reports_by_date 的查询行(末尾多一列 result)
fn map_detail_row(
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
fn collect_report_triples(rows: &[(ReportHistoryItem, Vec<i64>, String)]) -> (Vec<i64>, Vec<i64>) {
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

// ── scheduler helpers (used by scheduler.rs) ───────────────────────────

/// 供 scheduler 直接调用:读取全部定时任务(不经过 Tauri command 边界)
pub fn read_schedules(conn: &Connection) -> AppResult<Vec<ReportSchedule>> {
    let sql = format!("SELECT {SCHEDULE_COLS} FROM report_schedules");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], map_schedule_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 更新定时任务的 last_run_at。
#[allow(dead_code)]
pub fn update_last_run_at(conn: &Connection, schedule_id: &str, timestamp: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE report_schedules SET last_run_at = ?1 WHERE id = ?2",
        params![timestamp, schedule_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// 创建内存 SQLite(应用所有迁移),用于报告测试
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    /// 直接向 projects 表插入一行(绕过 `add` 的目录存在检查)
    fn insert_project(conn: &Connection, name: &str) -> i64 {
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO projects (path, name, description, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, ?3)",
            params![format!("/tmp/{name}-{now}"), name, now],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// 插入报告 + 关联 commits;commits_per_record 表示每个 project 的 commit 数。
    /// created_at 显式传入,避免连续插入时 created_at 相同导致排序无法断言。
    fn insert_report(
        conn: &Connection,
        project_ids: &[i64],
        date_from: &str,
        date_to: &str,
        period_type: &str,
        commits_per_record: usize,
    ) -> i64 {
        insert_report_with_ts(
            conn,
            project_ids,
            date_from,
            date_to,
            period_type,
            commits_per_record,
            chrono::Utc::now().timestamp(),
        )
    }

    /// 与 `insert_report` 类似,但允许指定 `created_at` 以便稳定断言排序。
    fn insert_report_with_ts(
        conn: &Connection,
        project_ids: &[i64],
        date_from: &str,
        date_to: &str,
        period_type: &str,
        commits_per_record: usize,
        created_at: i64,
    ) -> i64 {
        let ids_json = serde_json::to_string(project_ids).unwrap();
        conn.execute(
            "INSERT INTO report_history (project_ids, date_from, date_to, range_label,
                 author_mode, language, period_type, result, created_at)
             VALUES (?1, ?2, ?3, '', 'me', 'zh-CN', ?4, '', ?5)",
            params![ids_json, date_from, date_to, period_type, created_at],
        )
        .unwrap();
        let report_id = conn.last_insert_rowid();
        for pid in project_ids {
            let commits: Vec<GitCommitInfo> = (0..commits_per_record)
                .map(|i| GitCommitInfo {
                    hash: format!("h{i}"),
                    author: "tester".into(),
                    date: "2026-07-01 09:00".into(),
                    subject: format!("commit {i}"),
                })
                .collect();
            let commit_data = serde_json::to_string(&commits).unwrap();
            conn.execute(
                "INSERT INTO report_commits (report_id, project_id, project_name,
                     project_description, commit_data)
                 VALUES (?1, ?2, '', '', ?3)",
                params![report_id, pid, commit_data],
            )
            .unwrap();
        }
        report_id
    }

    #[test]
    fn list_filters_exact_project_id_not_substring() {
        // json_each 按 JSON 数组元素精确匹配项目 ID
        let conn = test_conn();
        let p1 = insert_project(&conn, "p1");
        let p12 = insert_project(&conn, "p12");
        let p123 = insert_project(&conn, "p123");

        insert_report(&conn, &[p1], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p12], "2026-07-02", "2026-07-02", "daily", 2);
        insert_report(&conn, &[p123], "2026-07-03", "2026-07-03", "daily", 3);

        let only_p12 = list_report_history_impl(&conn, None, None, Some(p12)).unwrap();
        assert_eq!(
            only_p12.len(),
            1,
            "筛选 12 必须只返回一条记录(不应包含 1/123)"
        );
        assert_eq!(only_p12[0].project_ids, vec![p12]);
        assert_eq!(only_p12[0].project_names, vec!["p12".to_string()]);
        assert_eq!(only_p12[0].total_commits, 2);

        let only_p1 = list_report_history_impl(&conn, None, None, Some(p1)).unwrap();
        assert_eq!(only_p1.len(), 1);
        assert_eq!(only_p1[0].project_ids, vec![p1]);

        let only_p123 = list_report_history_impl(&conn, None, None, Some(p123)).unwrap();
        assert_eq!(only_p123.len(), 1);
        assert_eq!(only_p123[0].project_ids, vec![p123]);

        // 不筛选应返回全部
        let all = list_report_history_impl(&conn, None, None, None).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn list_returns_descending_by_created_at() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        let r1 = insert_report_with_ts(
            &conn,
            &[p],
            "2026-07-01",
            "2026-07-01",
            "daily",
            1,
            1_000_000,
        );
        let r2 = insert_report_with_ts(
            &conn,
            &[p],
            "2026-07-02",
            "2026-07-02",
            "daily",
            1,
            2_000_000,
        );
        let r3 = insert_report_with_ts(
            &conn,
            &[p],
            "2026-07-03",
            "2026-07-03",
            "daily",
            1,
            3_000_000,
        );
        let items = list_report_history_impl(&conn, None, None, None).unwrap();
        assert_eq!(items.len(), 3);
        // 最后插入的(created_at 最大)应在第一位
        assert_eq!(items[0].id, r3);
        assert_eq!(items[2].id, r1);
        assert!(items[0].created_at >= items[1].created_at);
        assert!(items[1].created_at >= items[2].created_at);
        let _ = (r1, r2);
    }

    #[test]
    fn list_pagination_limit_offset() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        for _ in 0..5 {
            insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        }
        let page1 = list_report_history_impl(&conn, Some(2), Some(0), None).unwrap();
        let page2 = list_report_history_impl(&conn, Some(2), Some(2), None).unwrap();
        let page3 = list_report_history_impl(&conn, Some(2), Some(4), None).unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page3.len(), 1);
        // 三页 id 应互不重复
        let mut ids: Vec<i64> = page1
            .iter()
            .chain(page2.iter())
            .chain(page3.iter())
            .map(|i| i.id)
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn list_clamps_limit_to_200() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        let items = list_report_history_impl(&conn, Some(10_000), Some(0), None).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn list_no_filter_returns_correct_names_and_counts() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        insert_report(&conn, &[a, b], "2026-07-01", "2026-07-01", "daily", 3);
        let items = list_report_history_impl(&conn, None, None, None).unwrap();
        assert_eq!(items.len(), 1);
        // 名称按 NOCASE 排序:alpha, beta
        assert_eq!(
            items[0].project_names,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        // total_commits = 3 + 3 = 6
        assert_eq!(items[0].total_commits, 6);
    }

    #[test]
    fn calendar_meta_groups_by_date_to() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        // 2026-07-01: 2 份日报
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        // 2026-07-05: 1 份周报
        insert_report(&conn, &[p], "2026-06-29", "2026-07-05", "weekly", 2);
        // 2026-07-10: 0 份,不应出现在 map 中
        // 2026-07-15: 1 份日报
        insert_report(&conn, &[p], "2026-07-15", "2026-07-15", "daily", 1);
        // 7 月范围:2026-07-01 ~ 2026-07-31
        let dates = get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &None).unwrap();
        assert_eq!(dates.get("2026-07-01").copied(), Some(2));
        assert_eq!(dates.get("2026-07-05").copied(), Some(1));
        assert_eq!(dates.get("2026-07-15").copied(), Some(1));
        assert!(!dates.contains_key("2026-07-10"));
    }

    #[test]
    fn calendar_meta_filters_by_project() {
        let conn = test_conn();
        let p1 = insert_project(&conn, "p1");
        let p2 = insert_project(&conn, "p2");
        insert_report(&conn, &[p1], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p2], "2026-07-01", "2026-07-01", "daily", 1);
        let only_p1 = get_calendar_meta_impl(&conn, 2026, 7, &[p1], &[], &None).unwrap();
        assert_eq!(only_p1.get("2026-07-01").copied(), Some(1));
        let only_p2 = get_calendar_meta_impl(&conn, 2026, 7, &[p2], &[], &None).unwrap();
        assert_eq!(only_p2.get("2026-07-01").copied(), Some(1));
        let both = get_calendar_meta_impl(&conn, 2026, 7, &[p1, p2], &[], &None).unwrap();
        assert_eq!(both.get("2026-07-01").copied(), Some(2));
    }

    #[test]
    fn calendar_meta_includes_neighbour_month_padding() {
        // 锁住"前后月填充日也能拿到报告计数"的行为:
        // reka-ui CalendarRoot 把上月末尾与下月开头作为填充日一起渲染,
        // 后端查询区间必须覆盖整张网格,否则填充格的标注丢失。
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        // 2026-07-01 是周三,网格首格 = 2026-06-29(周一)
        insert_report(&conn, &[p], "2026-06-29", "2026-06-29", "daily", 1);
        // 下个月首周内的日期(2026-08-04 周二)
        insert_report(&conn, &[p], "2026-08-04", "2026-08-04", "daily", 1);
        // 当月内对照点
        insert_report(&conn, &[p], "2026-07-15", "2026-07-15", "daily", 1);
        // 远离网格的日期(2026-08-10 已超过 grid_end = 2026-08-09)
        insert_report(&conn, &[p], "2026-08-10", "2026-08-10", "daily", 1);

        let dates = get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &None).unwrap();

        // 前月填充日
        assert_eq!(dates.get("2026-06-29").copied(), Some(1));
        // 当月内对照点
        assert_eq!(dates.get("2026-07-15").copied(), Some(1));
        // 下月填充日
        assert_eq!(dates.get("2026-08-04").copied(), Some(1));
        // 超出网格末尾的日期不应出现
        assert!(!dates.contains_key("2026-08-10"));
    }

    #[test]
    fn calendar_meta_filters_by_report_type() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p], "2026-06-29", "2026-07-05", "weekly", 1);
        let only_daily =
            get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &Some("daily".into())).unwrap();
        assert_eq!(only_daily.get("2026-07-01").copied(), Some(1));
        assert!(!only_daily.contains_key("2026-07-05"));
        let only_weekly =
            get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &Some("weekly".into())).unwrap();
        assert_eq!(only_weekly.get("2026-07-05").copied(), Some(1));
        assert!(!only_weekly.contains_key("2026-07-01"));
    }

    #[test]
    fn reports_by_date_returns_commits_and_aggregates() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        // 同一天两条报告,使用显式 created_at 以稳定断言排序
        let r1 = insert_report_with_ts(
            &conn,
            &[a],
            "2026-07-01",
            "2026-07-01",
            "daily",
            2,
            1_000_000,
        );
        let r2 = insert_report_with_ts(
            &conn,
            &[b],
            "2026-07-01",
            "2026-07-01",
            "daily",
            3,
            2_000_000,
        );

        let details = get_reports_by_date_impl(&conn, "2026-07-01", &[], &[], &None).unwrap();
        assert_eq!(details.len(), 2);

        // 顺序按 created_at DESC: r2 在前
        let first = &details[0];
        let second = &details[1];
        let (first_id, second_id) = (first.item.id, second.item.id);
        assert_eq!(first_id, r2);
        assert_eq!(second_id, r1);

        // first(r2): project_names=[beta], total_commits=3, commits.len()=3
        assert_eq!(first.item.project_names, vec!["beta".to_string()]);
        assert_eq!(first.item.total_commits, 3);
        assert_eq!(first.commits.len(), 1);
        assert_eq!(first.commits[0].commits.len(), 3);

        // second(r1): project_names=[alpha], total_commits=2
        assert_eq!(second.item.project_names, vec!["alpha".to_string()]);
        assert_eq!(second.item.total_commits, 2);
        assert_eq!(second.commits.len(), 1);
        assert_eq!(second.commits[0].commits.len(), 2);
    }

    #[test]
    fn reports_by_date_filters_by_project() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        insert_report(&conn, &[a], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[b], "2026-07-01", "2026-07-01", "daily", 1);
        let only_a = get_reports_by_date_impl(&conn, "2026-07-01", &[a], &[], &None).unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].item.project_names, vec!["alpha".to_string()]);
    }

    #[test]
    fn batch_helpers_handle_empty_input() {
        let conn = test_conn();
        let names = resolve_project_names_batch(&conn, &[]).unwrap();
        assert!(names.is_empty());
        let counts = count_commits_batch(&conn, &[]).unwrap();
        assert!(counts.is_empty());
        let commits = load_report_commits_batch(&conn, &[]).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn list_report_history_empty_db() {
        let conn = test_conn();
        let items = list_report_history_impl(&conn, None, None, None).unwrap();
        assert!(items.is_empty());
        let items2 = list_report_history_impl(&conn, None, None, Some(1)).unwrap();
        assert!(items2.is_empty());
    }

    #[test]
    fn exact_project_id_filter_avoids_substring_matches() {
        let conn = test_conn();
        let now = chrono::Utc::now().timestamp();
        for (pid, label) in [(1i64, "p1"), (12, "p12"), (123, "p123")] {
            conn.execute(
                "INSERT INTO projects (id, path, name, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, '', ?4, ?4)",
                params![pid, format!("/tmp/{label}-{now}"), label, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO report_history (project_ids, date_from, date_to, range_label,
                     author_mode, language, period_type, result, created_at)
                 VALUES (?1, '2026-07-01', '2026-07-01', '', 'me', 'zh-CN', 'daily', '', ?2)",
                params![format!("[{pid}]"), now],
            )
            .unwrap();
        }

        let exact = list_report_history_impl(&conn, None, None, Some(1)).unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].project_ids, vec![1]);
    }
}
