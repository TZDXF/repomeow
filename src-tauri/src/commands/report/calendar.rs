use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};
use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use super::history::{
    collect_report_triples, count_commits_batch, load_report_commits_batch, map_detail_row,
    resolve_project_names_batch, ReportHistoryDetail,
};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::workday;

/// 日历标注数据：某月每天的报告数量 + 节假日/调休列表。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarMeta {
    pub dates: HashMap<String, i64>,
    pub holidays: Vec<String>,
    pub workdays: Vec<String>,
}

/// 为动态 WHERE 条件追加项目/标签/类型过滤(供日历与按日查询共用)。
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
    let (holidays, workdays) = workday::load_data(&workday::cache_root()).unwrap_or_default();

    Ok(CalendarMeta {
        dates,
        holidays: holidays.into_iter().collect(),
        workdays: workdays.into_iter().collect(),
    })
}

/// 日历查询覆盖周一开始的 6 × 7 月视图网格，包含相邻月份填充日。
pub fn get_calendar_meta_impl(
    conn: &Connection,
    year: i32,
    month: u32,
    project_ids: &[i64],
    tag_ids: &[i64],
    report_type: &Option<String>,
) -> AppResult<HashMap<String, i64>> {
    let month_start = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| {
        AppError::coded(
            ErrorCode::ReportInvalidYearMonth,
            format!("year={year} month={month}"),
        )
    })?;
    let grid_start =
        month_start - chrono::Duration::days(month_start.weekday().num_days_from_monday() as i64);
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

    let mut dates = HashMap::new();
    for row in rows {
        let (date, count) = row?;
        dates.insert(date, count);
    }
    Ok(dates)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HolidayData {
    pub holidays: Vec<String>,
    pub workdays: Vec<String>,
}

pub fn get_holiday_data() -> AppResult<HolidayData> {
    let (holidays, workdays) = workday::load_data(&workday::cache_root()).unwrap_or_default();
    Ok(HolidayData {
        holidays: holidays.into_iter().collect(),
        workdays: workdays.into_iter().collect(),
    })
}

pub fn get_reports_by_date(
    db: State<'_, Db>,
    date: String,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    let conn = db.0.lock().unwrap();
    get_reports_by_range_impl(&conn, &date, &date, &project_ids, &tag_ids, &report_type)
}

pub fn get_reports_by_range(
    db: State<'_, Db>,
    date_from: String,
    date_to: String,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    let conn = db.0.lock().unwrap();
    get_reports_by_range_impl(
        &conn,
        &date_from,
        &date_to,
        &project_ids,
        &tag_ids,
        &report_type,
    )
}

pub fn get_reports_by_range_impl(
    conn: &Connection,
    date_from: &str,
    date_to: &str,
    project_ids: &[i64],
    tag_ids: &[i64],
    report_type: &Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    let mut conditions = vec!["h.date_to BETWEEN ?1 AND ?2".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(date_from.to_string()),
        Box::new(date_to.to_string()),
    ];
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

    let (report_ids, project_id_set) = collect_report_triples(&rows);
    let name_map = resolve_project_names_batch(conn, &project_id_set)?;
    let count_map = count_commits_batch(conn, &report_ids)?;
    let commits_map = load_report_commits_batch(conn, &report_ids)?;

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
