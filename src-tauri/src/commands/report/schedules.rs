use std::sync::Arc;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Notify;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::workday;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSchedule {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_report_type")]
    pub report_type: String,
    pub project_ids: Vec<i64>,
    #[serde(default)]
    pub tag_ids: Vec<i64>,
    #[serde(default = "default_author_mode")]
    pub author_mode: String,
    pub time_of_day: String,
    #[serde(default)]
    pub weekdays_only: bool,
    #[serde(default)]
    pub chinese_workday_only: bool,
    #[serde(default = "default_previous_day")]
    pub previous_day: bool,
    #[serde(default = "default_weekly_workweek")]
    pub weekly_workweek: bool,
    #[serde(default = "default_weekly_start")]
    pub weekly_start_weekday: u32,
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

fn default_previous_day() -> bool {
    true
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

const SCHEDULE_COLS: &str = "id, name, enabled, report_type, project_ids, tag_ids, author_mode, \
     time_of_day, weekdays_only, chinese_workday_only, weekly_workweek, weekly_start_weekday, \
     weekly_end_weekday, last_run_at, previous_day";

fn map_schedule_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReportSchedule> {
    let ids_json: String = r.get(4)?;
    let tag_ids_json: String = r.get(5)?;
    Ok(ReportSchedule {
        id: r.get(0)?,
        name: r.get(1)?,
        enabled: r.get::<_, i64>(2)? != 0,
        report_type: r.get(3)?,
        project_ids: serde_json::from_str(&ids_json).unwrap_or_default(),
        tag_ids: serde_json::from_str(&tag_ids_json).unwrap_or_default(),
        author_mode: r.get(6)?,
        time_of_day: r.get(7)?,
        weekdays_only: r.get::<_, i64>(8)? != 0,
        chinese_workday_only: r.get::<_, i64>(9)? != 0,
        weekly_workweek: r.get::<_, i64>(10)? != 0,
        weekly_start_weekday: r.get::<_, u32>(11)?,
        weekly_end_weekday: r.get::<_, u32>(12)?,
        last_run_at: r.get(13)?,
        previous_day: r.get::<_, i64>(14)? != 0,
    })
}

pub fn list_report_schedules(db: State<'_, Db>) -> AppResult<Vec<ReportSchedule>> {
    let conn = db.0.lock().unwrap();
    read_schedules(&conn)
}

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
            let tag_ids_json = serde_json::to_string(&s.tag_ids).unwrap_or_default();
            tx.execute(
                "INSERT INTO report_schedules (id, name, enabled, report_type, project_ids, tag_ids, author_mode,
                     time_of_day, weekdays_only, chinese_workday_only, weekly_workweek,
                     weekly_start_weekday, weekly_end_weekday, last_run_at, previous_day)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    s.id,
                    s.name,
                    s.enabled as i64,
                    s.report_type,
                    ids_json,
                    tag_ids_json,
                    s.author_mode,
                    s.time_of_day,
                    s.weekdays_only as i64,
                    s.chinese_workday_only as i64,
                    s.weekly_workweek as i64,
                    s.weekly_start_weekday,
                    s.weekly_end_weekday,
                    s.last_run_at,
                    s.previous_day as i64,
                ],
            )?;
        }
        tx.commit()?;
    }

    if let Some(notify) = app.try_state::<ScheduleNotify>() {
        notify.0.notify_one();
    }
    Ok(())
}

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
    crate::scheduler::fire_schedule(&app, &data_dir, &schedule).await
}

pub struct ScheduleNotify(pub Arc<Notify>);

pub fn read_schedules(conn: &Connection) -> AppResult<Vec<ReportSchedule>> {
    let sql = format!("SELECT {SCHEDULE_COLS} FROM report_schedules");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], map_schedule_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn tag_project_ids(conn: &Connection, tag_ids: &[i64]) -> AppResult<Vec<i64>> {
    if tag_ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; tag_ids.len()].join(", ");
    let sql = format!(
        "SELECT DISTINCT pt.project_id FROM project_tags pt \
         JOIN projects p ON p.id = pt.project_id \
         WHERE pt.tag_id IN ({marks}) AND p.archived_at IS NULL \
         ORDER BY pt.project_id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(tag_ids.iter()), |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[allow(dead_code)]
pub fn update_last_run_at(conn: &Connection, schedule_id: &str, timestamp: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE report_schedules SET last_run_at = ?1 WHERE id = ?2",
        params![timestamp, schedule_id],
    )?;
    Ok(())
}
