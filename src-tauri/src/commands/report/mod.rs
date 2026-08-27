//! 日报/周报历史、日期规划与定时任务管理。
//!
//! 子模块按历史查询、日历、批量日期规划和计划任务存储拆分；本模块通过 re-export
//! 保持原有 `commands::report::*` 接口稳定。

mod calendar;
mod history;
mod planning;
mod schedules;

#[cfg(test)]
include!("tests.rs");

use tauri::{AppHandle, State};

use crate::db::Db;
use crate::error::AppResult;

pub use calendar::{CalendarDayReports, CalendarMeta, HolidayData};
pub(crate) use history::save_report_history_impl;
pub use history::{
    ReportGeneratedPayload, ReportHistoryDetail, ReportHistoryItem, SaveReportCommit,
};
pub use planning::{BatchRange, ReportDateRange, WorkWeekRanges};
pub use schedules::{read_schedules, tag_project_ids, ReportSchedule, ScheduleNotify};

#[cfg(test)]
pub use calendar::{get_calendar_meta_impl, get_reports_by_range_impl};
#[cfg(test)]
pub use history::{
    count_commits_batch, delete_report_history_impl, list_report_history_impl,
    load_report_commits_batch, resolve_project_names_batch,
};

#[tauri::command]
pub fn list_report_history(
    db: State<'_, Db>,
    limit: Option<usize>,
    offset: Option<usize>,
    project_id: Option<i64>,
) -> AppResult<Vec<ReportHistoryItem>> {
    history::list_report_history(db, limit, offset, project_id)
}

#[tauri::command]
pub fn get_report_history(db: State<'_, Db>, id: i64) -> AppResult<ReportHistoryDetail> {
    history::get_report_history(db, id)
}

#[tauri::command]
pub fn delete_report_history(db: State<'_, Db>, id: i64) -> AppResult<()> {
    history::delete_report_history(db, id)
}

#[tauri::command]
pub fn get_calendar_meta(
    db: State<'_, Db>,
    year: i32,
    month: u32,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<CalendarMeta> {
    calendar::get_calendar_meta(db, year, month, project_ids, tag_ids, report_type)
}

#[tauri::command]
pub fn get_holiday_data() -> AppResult<HolidayData> {
    calendar::get_holiday_data()
}

#[tauri::command]
pub fn get_reports_by_date(
    db: State<'_, Db>,
    date: String,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    calendar::get_reports_by_date(db, date, project_ids, tag_ids, report_type)
}

#[tauri::command]
pub fn get_reports_by_range(
    db: State<'_, Db>,
    date_from: String,
    date_to: String,
    project_ids: Vec<i64>,
    tag_ids: Vec<i64>,
    report_type: Option<String>,
) -> AppResult<Vec<ReportHistoryDetail>> {
    calendar::get_reports_by_range(db, date_from, date_to, project_ids, tag_ids, report_type)
}

#[tauri::command]
pub async fn get_work_week_ranges() -> AppResult<WorkWeekRanges> {
    planning::get_work_week_ranges().await
}

#[tauri::command]
pub async fn plan_batch_report_ranges(
    period_type: String,
    date_from: String,
    date_to: String,
) -> AppResult<Vec<BatchRange>> {
    planning::plan_batch_report_ranges(period_type, date_from, date_to).await
}

#[tauri::command]
pub fn list_report_dates(
    db: State<'_, Db>,
    period_type: String,
    date_from: String,
    date_to: String,
) -> AppResult<Vec<ReportDateRange>> {
    planning::list_report_dates(db, period_type, date_from, date_to)
}

#[tauri::command]
pub fn list_report_schedules(db: State<'_, Db>) -> AppResult<Vec<ReportSchedule>> {
    schedules::list_report_schedules(db)
}

#[tauri::command]
pub fn save_report_schedules(
    db: State<'_, Db>,
    app: AppHandle,
    schedules: Vec<ReportSchedule>,
) -> AppResult<()> {
    schedules::save_report_schedules(db, app, schedules)
}

#[tauri::command]
pub async fn run_report_schedule_now(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
) -> AppResult<i64> {
    schedules::run_report_schedule_now(app, db, id).await
}
