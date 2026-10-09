use super::*;
use crate::agent::types::AgentTool;
use crate::commands::script;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::CustomCommand;
use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

// ── 自定义命令 ───────────────────────────────────────────────────────

/// 按名称定位当前项目的自定义命令(项目内名称唯一,冲突在写入侧报 CommandNameConflict)。
fn find_command_by_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> AppResult<CustomCommand> {
    script::list_commands(conn, project_id)?
        .into_iter()
        .find(|command| command.name == name)
        .ok_or_else(|| AppError::coded(ErrorCode::CommandNotFound, name.to_string()))
}

/// 可选字符串参数:未传返回 None;传了但为空白视为无效参数。
pub(super) fn non_empty_opt(args: &Value, key: &str) -> Result<Option<String>, ToolExecutionError> {
    match arg_str(args, key) {
        Some(value) if !value.trim().is_empty() => Ok(Some(value.trim().to_string())),
        Some(_) => Err(tool_err(invalid_arg(key))),
        None => Ok(None),
    }
}

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

pub(super) fn update_custom_command_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "update_custom_command",
        "编辑自定义命令",
        "修改当前项目已登记的自定义命令,按 name 定位(名称见 list_custom_commands 输出)。仅在用户明确要求「修改/重命名」某条命令时使用;当前为「确认后执行」权限时,应用会在执行前弹出确认,不必在正文中先征得同意,但应说明将改成的内容。未提供的字段保持原值;description 传空字符串表示清空描述。参数:name(必填)要修改的命令名称;command(可选)新的命令文本;description(可选)新的用途说明;new_name(可选)重命名后的新名称。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "要修改的命令名称(当前名称,项目内唯一)。"
                },
                "command": {
                    "type": "string",
                    "description": "新的命令文本,可选;不传保持原值。"
                },
                "description": {
                    "type": "string",
                    "description": "新的用途说明,可选;不传保持原值,传空字符串清空。"
                },
                "new_name": {
                    "type": "string",
                    "description": "重命名后的新名称,可选。"
                }
            },
            "required": ["name"],
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
                    let new_command = non_empty_opt(&args, "command")?;
                    let new_name = non_empty_opt(&args, "new_name")?;
                    // description 未传(null/缺省)保持原值;传了(含空串)即采用,空串等于清空。
                    let new_description = match args.get("description") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(value)) => Some(value.trim().to_string()),
                        Some(_) => return Err(tool_err(invalid_arg("description"))),
                    };
                    if new_name.is_none() && new_command.is_none() && new_description.is_none() {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::AiRequestFailed,
                            "至少提供 command / description / new_name 中的一个要修改的字段",
                        )));
                    }
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            project_path,
                        )));
                    };
                    let db = app.state::<Db>();
                    let updated = {
                        let conn = db.0.lock().unwrap();
                        let existing =
                            find_command_by_name(&conn, project_id, &name).map_err(tool_err)?;
                        let command_id = existing.id;
                        let icon = existing.icon;
                        let name_after = new_name.unwrap_or(existing.name);
                        let command_after = new_command.unwrap_or(existing.command);
                        let description_after = new_description.unwrap_or(existing.description);
                        script::update_command(
                            &conn,
                            command_id,
                            &name_after,
                            &command_after,
                            &description_after,
                            &icon,
                        )
                        .map_err(tool_err)?
                    };
                    // 编辑可能同步了「常用命令」标记快照,广播让另一窗口刷新
                    let _ = app.emit("projects://pins-changed", serde_json::json!({}));
                    script::emit_custom_commands_changed(&app);
                    text_result(format!(
                        "已更新自定义命令「{}」:`{}`{}",
                        updated.name,
                        updated.command,
                        if updated.description.is_empty() {
                            String::new()
                        } else {
                            format!("({})", updated.description)
                        }
                    ))
                })
            }
        },
    )
}

pub(super) fn delete_custom_command_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "delete_custom_command",
        "删除自定义命令",
        "删除当前项目已登记的自定义命令,按 name 定位(名称见 list_custom_commands 输出)。仅在用户明确要求「删除」某条命令时使用;当前为「确认后执行」权限时,应用会在执行前弹出确认,不必在正文中先征得同意,但应说明将删除哪条命令。删除会连带移除该命令的「常用命令」标记。参数:name(必填)要删除的命令名称。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "要删除的命令名称(项目内唯一)。"
                }
            },
            "required": ["name"],
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
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            project_path,
                        )));
                    };
                    let db = app.state::<Db>();
                    let deleted = {
                        let conn = db.0.lock().unwrap();
                        let existing = find_command_by_name(&conn, project_id, &name).map_err(tool_err)?;
                        script::delete_command(&conn, existing.id).map_err(tool_err)?;
                        existing
                    };
                    // 删除会连带移除「常用命令」标记,广播让另一窗口刷新
                    let _ = app.emit("projects://pins-changed", serde_json::json!({}));
                    script::emit_custom_commands_changed(&app);
                    text_result(format!(
                        "已删除自定义命令「{}」:`{}`",
                        deleted.name, deleted.command
                    ))
                })
            }
        },
    )
}
