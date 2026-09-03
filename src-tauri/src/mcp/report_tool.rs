use chrono::Local;

use serde_json::{json, Value};

use crate::commands::ai::{mcp_generate_and_save_report, GenerateAndSaveReportRequest};

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
        run_id: format!("mcp-{}", crate::time_util::now_ts_nanos()),
        project_ids,
        date_from,
        date_to,
        range_label: range_label.clone(),
        author_mode: author_mode.to_string(),
        language: language.to_string(),
        period_type: period_type.to_string(),
    };
    let Some(report) = mcp_generate_and_save_report(&data_root, &db, &request)
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
