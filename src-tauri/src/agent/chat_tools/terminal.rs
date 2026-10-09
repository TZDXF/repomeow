use super::*;
use crate::commands::terminal::{
    get_command_session_output_impl, list_command_sessions_impl, restart_command_session_impl,
    run_command_session_impl, stop_command_session_impl, TerminalManager,
};
use crate::error::{AppError, ErrorCode};
use crate::models::TerminalSessionInfo;
use serde_json::json;
use std::path::Path;
use tauri::{Manager, State};

// ── 共享辅助 ─────────────────────────────────────────────────────────

fn manager(app: &AppHandle) -> State<'_, std::sync::Arc<TerminalManager>> {
    app.state::<std::sync::Arc<TerminalManager>>()
}

fn project_sessions(app: &AppHandle, project_id: i64) -> Vec<TerminalSessionInfo> {
    list_command_sessions_impl(&manager(app).inner().clone())
        .into_iter()
        .filter(|session| session.project_id == project_id)
        .collect()
}

fn project_session(
    app: &AppHandle,
    project_id: i64,
    id: u64,
) -> Result<TerminalSessionInfo, ToolExecutionError> {
    project_sessions(app, project_id)
        .into_iter()
        .find(|session| session.id == id)
        .ok_or_else(|| {
            tool_err(AppError::coded(
                ErrorCode::TerminalSessionNotFound,
                id.to_string(),
            ))
        })
}

/// 把终端输出转成适合 LLM 的纯文本:PTY 序列大多是控制字符,直接读会污染上下文。
fn clean_terminal_output(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect()
}

/// 解析创建 tab 的工作目录:绝对路径或项目相对路径都必须落在项目根内。
fn resolve_ai_cwd(project_path: &str, cwd: Option<&str>) -> Result<String, ToolExecutionError> {
    let root = Path::new(project_path);
    let root_canonical = root.canonicalize().map_err(|e| {
        tool_err(AppError::coded(
            ErrorCode::ScriptDirNotFound,
            format!("{project_path}: {e}"),
        ))
    })?;
    let candidate = match cwd.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = Path::new(value);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        }
        None => root.to_path_buf(),
    };
    let canonical = candidate.canonicalize().map_err(|e| {
        tool_err(AppError::coded(
            ErrorCode::ScriptDirNotFound,
            format!("{}: {e}", candidate.display()),
        ))
    })?;
    if !canonical.starts_with(&root_canonical) || !canonical.is_dir() {
        return Err(tool_err(AppError::coded(
            ErrorCode::InvalidPath,
            candidate.display().to_string(),
        )));
    }
    Ok(crate::path_util::clean_str(&canonical.to_string_lossy()))
}

fn session_metadata_text(session: &TerminalSessionInfo) -> String {
    pretty_json(session)
}

// ── 终端 Tab 读取 ────────────────────────────────────────────────────

pub(super) fn list_terminal_tabs_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "list_terminal_tabs",
        "终端 Tab 清单",
        "列出当前项目的内嵌终端 Tab(id/命令/工作目录/运行状态/退出码)。需要读取或操作终端前先调用;无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |_args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let Some(project_id) = project_id else {
                        return text_result("当前项目未在 RepoMeow 登记,无法读取内嵌终端 Tab。");
                    };
                    let sessions = project_sessions(&app, project_id);
                    if sessions.is_empty() {
                        return text_result("当前项目暂无内嵌终端 Tab。");
                    }
                    text_result(pretty_json(&sessions))
                })
            }
        },
    )
}

pub(super) fn read_terminal_tab_output_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "read_terminal_tab_output",
        "读终端输出",
        "读取指定内嵌终端 Tab 的当前输出缓冲(最多保留最近输出,会剥离控制序列)。用于查看命令结果、日志或交互 Shell 屏幕;参数:id(必填,来自 list_terminal_tabs)。",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "终端 Tab ID。"
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            "项目未登记",
                        )));
                    };
                    let id =
                        arg_u64_opt(&args, "id")?.ok_or_else(|| tool_err(invalid_arg("id")))?;
                    let session = project_session(&app, project_id, id)?;
                    let output =
                        get_command_session_output_impl(&manager(&app).inner().clone(), id)
                            .map_err(tool_err)?;
                    let output = clean_terminal_output(&output);
                    let body = if output.is_empty() {
                        "(暂无输出;运行中的命令可稍后再读取)".to_string()
                    } else {
                        truncate_bytes(output, TOOL_RESULT_MAX_BYTES)
                    };
                    text_result(format!(
                        "{}\n\n<terminal_output>\n{body}\n</terminal_output>",
                        session_metadata_text(&session)
                    ))
                })
            }
        },
    )
}

// ── 终端 Tab 写操作 ──────────────────────────────────────────────────

pub(super) fn create_terminal_tab_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "create_terminal_tab",
        "创建终端 Tab 执行命令",
        "在当前项目的内嵌终端中新建命令 Tab 并执行 command;命令异步运行,输出会推送到用户可见终端。返回 Tab ID,稍后用 read_terminal_tab_output 查看结果;需要停止/重启时记住 ID。参数:command(必填);cwd(可选,项目相对或项目内绝对路径);label(可选展示名)。",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "minLength": 1,
                    "description": "要在终端执行的命令文本。"
                },
                "cwd": {
                    "type": "string",
                    "description": "工作目录,可选;必须是项目根或其子目录。"
                },
                "label": {
                    "type": "string",
                    "description": "终端页签展示名,可选。"
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            let project_name = ctx.project_name.clone();
            let project_path = ctx.work_dir();
            move |args, _on_update| {
                let app = app.clone();
                let project_name = project_name.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let project_id = project_id.ok_or_else(|| {
                        tool_err(AppError::coded(ErrorCode::ProjectNotFound, &project_path))
                    })?;
                    let command = require_str(&args, "command")?;
                    let cwd_input = non_empty_opt(&args, "cwd")?;
                    let label = non_empty_opt(&args, "label")?;
                    let cwd = resolve_ai_cwd(&project_path, cwd_input.as_deref())?;
                    let session = run_command_session_impl(
                        &app,
                        &manager(&app).inner().clone(),
                        project_id,
                        &project_name,
                        &project_path,
                        &command,
                        Some(&cwd),
                        None,
                        Some("ai"),
                        label.as_deref(),
                    )
                    .map_err(tool_err)?;
                    text_result(format!(
                        "已创建终端 Tab 并开始执行命令:\n{}",
                        session_metadata_text(&session)
                    ))
                })
            }
        },
    )
}

pub(super) fn stop_terminal_tab_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "stop_terminal_tab",
        "停止终端 Tab",
        "停止当前项目指定的内嵌终端 Tab(Windows 下停止整棵进程树)。参数:id(必填,来自 list_terminal_tabs)。",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "要停止的终端 Tab ID。"
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            "项目未登记",
                        )));
                    };
                    let id =
                        arg_u64_opt(&args, "id")?.ok_or_else(|| tool_err(invalid_arg("id")))?;
                    let session = project_session(&app, project_id, id)?;
                    if session.status != crate::models::TerminalSessionStatus::Running {
                        return text_result(format!(
                            "终端 Tab #{id} 已不在运行中(状态 {}),无需停止。",
                            serde_json::to_value(&session)
                                .ok()
                                .and_then(|value| value
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string))
                                .unwrap_or_else(|| "unknown".into())
                        ));
                    }
                    stop_command_session_impl(&manager(&app).inner().clone(), id)
                        .map_err(tool_err)?;
                    text_result(format!(
                        "已发送终端 Tab #{id} 的停止请求;状态会在进程退出后更新。"
                    ))
                })
            }
        },
    )
}

pub(super) fn restart_terminal_tab_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "restart_terminal_tab",
        "重启终端 Tab",
        "以原命令和工作目录重启当前项目指定的内嵌终端 Tab;输出缓冲会清空且 Tab ID 不变。参数:id(必填,来自 list_terminal_tabs)。",
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "要重启的终端 Tab ID。"
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            "项目未登记",
                        )));
                    };
                    let id =
                        arg_u64_opt(&args, "id")?.ok_or_else(|| tool_err(invalid_arg("id")))?;
                    project_session(&app, project_id, id)?;
                    let session =
                        restart_command_session_impl(&app, &manager(&app).inner().clone(), id)
                            .map_err(tool_err)?;
                    text_result(format!(
                        "已重启终端 Tab:\n{}",
                        session_metadata_text(&session)
                    ))
                })
            }
        },
    )
}
