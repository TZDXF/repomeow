use chrono::{Local, TimeZone};
use serde_json::{json};
use tauri::{AppHandle, Manager};
use crate::agent::types::{AgentTool, AgentToolResult};
use crate::commands::ai::{ai_generate_and_save_report, GenerateAndSaveReportRequest};
use crate::commands::report;
use crate::db::Db;
use crate::error::{AppError, ErrorCode};
use super::*;

// ── 报告 ─────────────────────────────────────────────────────────────

pub(super) fn generate_report_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "generate_report",
        "生成日报/周报",
        "生成项目日报或周报:汇总指定时间范围的 git 提交,调用 AI 生成正文并保存到报告历史(同步等待,通常十几秒)。仅在用户要求生成日报/周报时使用;当前为「确认后执行」权限时,应用会在执行前弹出确认,不必在正文中先征得同意。参数:period_type(必填)\"daily\" 或 \"weekly\";date_from/date_to(可选)\"YYYY-MM-DD\",缺省 daily=今天、weekly=最近 7 天;author_mode(可选)\"all\" 统计所有人、\"me\" 仅当前 git 用户,缺省 \"all\"。返回报告正文(超长截断)。",
        json!({
            "type": "object",
            "properties": {
                "period_type": {
                    "type": "string",
                    "enum": ["daily", "weekly"],
                    "description": "报告类型:daily 日报 / weekly 周报。"
                },
                "date_from": {
                    "type": "string",
                    "description": "起始日期 YYYY-MM-DD,可选。"
                },
                "date_to": {
                    "type": "string",
                    "description": "结束日期 YYYY-MM-DD,可选。"
                },
                "author_mode": {
                    "type": "string",
                    "enum": ["all", "me"],
                    "description": "提交作者范围:all 全部 / me 仅当前 git 用户,可选。"
                }
            },
            "required": ["period_type"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            let project_path = ctx.project_path.clone();
            move |args, on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let period_type = match arg_str(&args, "period_type") {
                        Some("daily") => "daily",
                        Some("weekly") => "weekly",
                        _ => return Err(tool_err(invalid_arg("period_type (daily|weekly)"))),
                    }
                    .to_string();
                    let today = Local::now().date_naive();
                    let default_from = if period_type == "weekly" {
                        today - chrono::Duration::days(6)
                    } else {
                        today
                    };
                    let date_from = arg_str(&args, "date_from")
                        .map(str::to_string)
                        .unwrap_or_else(|| default_from.format("%Y-%m-%d").to_string());
                    let date_to = arg_str(&args, "date_to")
                        .map(str::to_string)
                        .unwrap_or_else(|| today.format("%Y-%m-%d").to_string());
                    let range_label = if period_type == "weekly" && date_from != date_to {
                        format!("{date_from} ~ {date_to}")
                    } else {
                        date_from.clone()
                    };
                    let author_mode = match arg_str(&args, "author_mode") {
                        Some("me") => "me".to_string(),
                        _ => "all".to_string(),
                    };
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            project_path,
                        )));
                    };
                    if let Some(on_update) = &on_update {
                        on_update(AgentToolResult::text("正在收集提交并生成报告…"));
                    }
                    let request = GenerateAndSaveReportRequest {
                        run_id: pseudo_request_id(),
                        project_ids: vec![project_id],
                        date_from,
                        date_to,
                        range_label,
                        author_mode,
                        language: "zh-CN".to_string(),
                        period_type,
                    };
                    let db = app.state::<Db>();
                    match ai_generate_and_save_report(app.clone(), db, request)
                        .await
                        .map_err(tool_err)?
                    {
                        None => text_result("所选时间范围内没有提交记录,未生成报告。"),
                        Some(report) => text_result(truncate_bytes(
                            format!(
                                "报告已生成并保存(历史 id:{})。\n\n{}",
                                report.history_id, report.result
                            ),
                            REPORT_RESULT_MAX_BYTES,
                        )),
                    }
                })
            }
        },
    )
}

pub(super) fn list_reports_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "list_reports",
        "报告历史",
        "列出本项目最近的报告历史(按生成时间倒序)。用户问「之前的报告」「上次周报」时使用,把条目信息(时间范围/类型/提交数)转述给用户即可。参数:limit(可选)返回条数,默认 10。",
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "返回条数(1-50),默认 10。"
                }
            },
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let limit = arg_u64_opt(&args, "limit")?
                        .unwrap_or(10)
                        .clamp(1, 50) as usize;
                    let db = app.state::<Db>();
                    let items = report::list_report_history(db, Some(limit), Some(0), project_id)
                        .map_err(tool_err)?;
                    if items.is_empty() {
                        return text_result("本项目还没有报告历史。");
                    }
                    text_result(
                        items
                            .iter()
                            .map(|item| {
                                let generated_at = Local
                                    .timestamp_opt(item.created_at, 0)
                                    .single()
                                    .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_default();
                                let kind = if item.period_type == "weekly" {
                                    "周报"
                                } else {
                                    "日报"
                                };
                                format!(
                                    "- [{}] {}({},{},{} 条提交,生成于 {generated_at})",
                                    item.id,
                                    item.range_label,
                                    kind,
                                    item.project_names.join("、"),
                                    item.total_commits
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
            }
        },
    )
}


