use chrono::Local;

use serde_json::{json, Value};

use crate::commands::ai::{cli_generate_and_save_report, GenerateAndSaveReportRequest};

use super::types::GenerateReportInput;
use super::util::{data_root_or_default, open_db, resolve_project_id, truncate_text, ToolFailure};
use crate::path_util::clean_str;
use std::path::Path;

/// generate_report 返回正文的字节上限(对齐 chat 工具)。
const REPORT_RESULT_MAX_BYTES: usize = 4 * 1024;

// ── 报告生成 ──────────────────────────────────────────────────────────

pub(super) async fn generate_report_impl(
    input: GenerateReportInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let period_type = match input.period_type.trim() {
        "daily" => "daily",
        "weekly" => "weekly",
        _ => {
            return Err(ToolFailure::new(
                "invalid_period_type",
                "periodType 必须是 daily 或 weekly",
            ))
        }
    };
    if input.project_directories.is_empty() {
        return Err(ToolFailure::new(
            "project_directories_required",
            "projectDirectories 至少需要一个项目目录",
        ));
    }
    let today = Local::now().date_naive();
    let default_from = if period_type == "weekly" {
        today - chrono::Duration::days(6)
    } else {
        today
    };
    let date_from = input
        .date_from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_from.format("%Y-%m-%d").to_string());
    let date_to = input
        .date_to
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| today.format("%Y-%m-%d").to_string());
    let range_label = if date_from != date_to {
        format!("{date_from} ~ {date_to}")
    } else {
        date_from.clone()
    };
    let author_mode = match input.author_mode.as_deref() {
        Some("me") => "me",
        _ => "all",
    };
    let language = match input.language.as_deref() {
        Some("en-US") => "en-US",
        _ => "zh-CN",
    };

    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let mut project_ids: Vec<i64> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    {
        let conn = db.0.lock().unwrap();
        for directory in &input.project_directories {
            match resolve_project_id(&conn, directory)? {
                Some(id) if !project_ids.contains(&id) => project_ids.push(id),
                Some(_) => {}
                None => unknown.push(clean_str(directory)),
            }
        }
    }
    if !unknown.is_empty() {
        return Err(
            ToolFailure::new("project_not_found", "以下目录未在 RepoMeow 登记或已归档")
                .with_detail(unknown.join("; ")),
        );
    }

    let request = GenerateAndSaveReportRequest {
        run_id: format!("cli-{}", crate::time_util::now_ts_nanos()),
        project_ids,
        date_from,
        date_to,
        range_label: range_label.clone(),
        author_mode: author_mode.to_string(),
        language: language.to_string(),
        period_type: period_type.to_string(),
    };
    let Some(report) = cli_generate_and_save_report(&data_root, &db, &request)
        .await
        .map_err(|error| ToolFailure::from_app("生成报告失败", error))?
    else {
        return Ok(json!({
            "generated": false,
            "rangeLabel": range_label,
            "message": "所选时间范围内没有提交记录,未生成报告。",
        }));
    };
    let (result, result_truncated) = truncate_text(&report.result, REPORT_RESULT_MAX_BYTES);
    Ok(json!({
        "generated": true,
        "historyId": report.history_id,
        "rangeLabel": range_label,
        "result": result,
        "resultTruncated": result_truncated,
        "projects": report
            .commit_data
            .iter()
            .map(|project| json!({
                "name": project.project_name,
                "commits": project.commits.len(),
            }))
            .collect::<Vec<_>>(),
    }))
}

// ── 报告历史与调度管理 ─────────────────────────────────────────────────

/// 读取单条报告详情(含 Markdown 正文与提交记录)。
pub(super) fn get_report_impl(id: i64, data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let detail = crate::commands::report::get_report_history_impl(&conn, id)
        .map_err(|error| ToolFailure::from_app("查询报告详情失败", error))?;
    Ok(json!(detail))
}

/// 删除单条报告历史(级联删除关联提交记录)。
pub(super) fn delete_report_impl(id: i64, data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    crate::commands::report::delete_report_history_impl(&conn, id)
        .map_err(|error| ToolFailure::from_app("删除报告失败", error))?;
    Ok(json!({ "id": id, "deleted": true }))
}

/// 列出全部报告调度。
pub(super) fn list_schedules_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let schedules = crate::commands::report::read_schedules(&conn)
        .map_err(|error| ToolFailure::from_app("查询报告调度失败", error))?;
    Ok(json!({ "schedules": schedules }))
}

/// 从 JSON 文件全量覆盖报告调度(数组,元素结构同 schedules 输出)。
/// 注意:若桌面应用正在运行,需重启后新调度才被调度器感知。
pub(super) fn save_schedules_impl(
    file: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let raw = std::fs::read_to_string(file).map_err(|error| {
        ToolFailure::new("schedule_file_read_failed", "读取调度 JSON 文件失败")
            .with_detail(format!("{file}: {error}"))
    })?;
    let schedules: Vec<crate::commands::report::ReportSchedule> = serde_json::from_str(&raw)
        .map_err(|error| {
            ToolFailure::new("schedule_file_invalid", "调度 JSON 格式无效")
                .with_detail(error.to_string())
        })?;
    let count = schedules.len();
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let mut conn = db.0.lock().unwrap();
    crate::commands::report::write_schedules(&mut conn, &schedules)
        .map_err(|error| ToolFailure::from_app("保存报告调度失败", error))?;
    Ok(json!({ "saved": count }))
}

/// 列出系统级调度(当前仅 git_update 后台检查)。
pub(super) fn list_system_schedules_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let schedule = crate::commands::git::read_git_system_schedule(&conn)
        .map_err(|error| ToolFailure::from_app("查询系统调度失败", error))?;
    Ok(json!({ "schedules": [schedule] }))
}

/// 保存系统级调度(git_update)。注意:运行中的桌面应用需重启后生效。
pub(super) fn save_system_schedule_impl(
    enabled: bool,
    interval_minutes: u64,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let schedule =
        crate::commands::git::write_git_system_schedule(&conn, enabled, interval_minutes)
            .map_err(|error| ToolFailure::from_app("保存系统调度失败", error))?;
    Ok(json!(schedule))
}
