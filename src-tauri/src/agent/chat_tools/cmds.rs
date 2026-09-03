use serde_json::json;
use tauri::{AppHandle, Manager};
use crate::agent::types::AgentTool;
use crate::commands::script;
use crate::db::Db;
use crate::error::{AppError, ErrorCode};
use super::*;

// ── 自定义命令 ───────────────────────────────────────────────────────

pub(super) fn list_custom_commands_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "list_custom_commands",
        "自定义命令清单",
        "列出当前项目已登记的自定义命令(名称/命令文本/描述)。用户问「有哪些自定义命令」「怎么跑 XX」时使用;需要新增命令时配合 add_custom_command。无参数。",
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
                        return text_result(
                            "当前项目未在 RepoMeow 登记(无 project_id),无法管理自定义命令。",
                        );
                    };
                    let db = app.state::<Db>();
                    let commands = {
                        let conn = db.0.lock().unwrap();
                        script::list_commands(&conn, project_id).map_err(tool_err)?
                    };
                    if commands.is_empty() {
                        return text_result("该项目暂无自定义命令。");
                    }
                    text_result(
                        commands
                            .iter()
                            .map(|command| {
                                if command.description.is_empty() {
                                    format!("- {}:`{}`", command.name, command.command)
                                } else {
                                    format!(
                                        "- {}:`{}`({})",
                                        command.name, command.command, command.description
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
            }
        },
    )
}

pub(super) fn add_custom_command_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "add_custom_command",
        "新增自定义命令",
        "为当前项目新增一条自定义命令(保存到 RepoMeow,用户可在界面一键在终端执行)。仅在用户明确要求「添加/保存命令」时使用;当前为「确认后执行」权限时,应用会在执行前弹出确认,不必在正文中先征得同意,但应说明将写入的内容。参数:name(必填)命令名称;command(必填)将在终端执行的命令文本;description(可选)用途说明。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "命令名称(项目内唯一)。"
                },
                "command": {
                    "type": "string",
                    "description": "将在终端执行的命令文本。"
                },
                "description": {
                    "type": "string",
                    "description": "用途说明,可选。"
                }
            },
            "required": ["name", "command"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            let project_path = ctx.project_path.clone();
            move |args, _on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let name = require_str(&args, "name")?;
                    let command = require_str(&args, "command")?;
                    let description =
                        arg_str(&args, "description").unwrap_or_default().trim().to_string();
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            project_path,
                        )));
                    };
                    let db = app.state::<Db>();
                    let created = {
                        let conn = db.0.lock().unwrap();
                        script::create_command(&conn, project_id, &name, &command, &description, "")
                            .map_err(tool_err)?
                    };
                    // 通知详情页 CustomCommands 卡片刷新,新建命令立即可见
                    script::emit_custom_commands_changed(&app);
                    text_result(format!(
                        "已创建自定义命令「{}」:`{}`",
                        created.name, created.command
                    ))
                })
            }
        },
    )
}


