//! CLI 与桌面进程之间的一次性终端控制握手。
//!
//! CLI 将请求写入应用数据目录并以内部参数拉起当前可执行文件。已有实例由
//! single-instance 插件接收参数;首个实例则在 setup 后处理同一组参数。结果通过
//! 临时文件原子写回,避免额外监听端口或创建脱离应用窗口的后台终端。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::commands::terminal::{
    self, list_command_sessions_impl, run_command_session_impl, stop_command_session_impl,
    write_command_session_impl, TerminalManager,
};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::TerminalSessionInfo;
use crate::path_util::clean_str;

pub(crate) const TERMINAL_IPC_ARG: &str = "--repomeow-terminal-ipc";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum TerminalCommandRef {
    Npm { package: String, script: String },
    Custom { id: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum TerminalIpcRequest {
    List {
        project_directory: Option<String>,
    },
    Output {
        id: u64,
        project_directory: Option<String>,
    },
    Input {
        id: u64,
        project_directory: Option<String>,
        text: String,
        enter: bool,
    },
    Create {
        project_directory: String,
        command: TerminalCommandRef,
    },
    Start {
        id: u64,
        project_directory: Option<String>,
    },
    Stop {
        id: u64,
        project_directory: Option<String>,
    },
    Restart {
        id: u64,
        project_directory: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TerminalIpcResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl TerminalIpcResponse {
    fn success(result: Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            code: None,
            message: None,
            detail: None,
        }
    }

    fn error(code: impl Into<String>, message: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            ok: false,
            result: None,
            code: Some(code.into()),
            message: Some(message.into()),
            detail,
        }
    }

    fn from_app(message: &str, error: AppError) -> Self {
        Self::error(error.code(), message, Some(error.to_string()))
    }
}

/// single-instance 回调与首个实例 setup 共用的入口。非内部参数直接忽略。
pub(crate) fn handle_desktop_args(app: &AppHandle, args: &[String]) {
    let Some(index) = args.iter().position(|arg| arg == TERMINAL_IPC_ARG) else {
        return;
    };
    let (Some(request_path), Some(response_path)) = (args.get(index + 1), args.get(index + 2))
    else {
        return;
    };
    let request_path = PathBuf::from(request_path);
    let response_path = PathBuf::from(response_path);
    let request = fs::read_to_string(&request_path)
        .map_err(|error| {
            TerminalIpcResponse::error(
                "terminal_ipc_read_failed",
                "读取终端控制请求失败",
                Some(error.to_string()),
            )
        })
        .and_then(|raw| {
            serde_json::from_str::<TerminalIpcRequest>(&raw).map_err(|error| {
                TerminalIpcResponse::error(
                    "terminal_ipc_invalid_request",
                    "终端控制请求格式无效",
                    Some(error.to_string()),
                )
            })
        });
    let _ = fs::remove_file(&request_path);
    let response = match request {
        Ok(request) => match dispatch(app, request) {
            Ok(result) => TerminalIpcResponse::success(result),
            Err(error) => TerminalIpcResponse::from_app("终端 Tab 操作失败", error),
        },
        Err(response) => response,
    };
    if let Err(error) = write_response(&response_path, &response) {
        eprintln!("[terminal-ipc] 写回 CLI 响应失败: {error}");
    }
}

fn write_response(path: &Path, response: &TerminalIpcResponse) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec(response)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn clean_terminal_output(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect()
}

fn dispatch(app: &AppHandle, request: TerminalIpcRequest) -> AppResult<Value> {
    let manager = app.state::<Arc<TerminalManager>>().inner().clone();
    match request {
        TerminalIpcRequest::List { project_directory } => {
            let project_id = project_directory
                .as_deref()
                .map(|directory| resolve_project(app, directory).map(|project| project.0))
                .transpose()?;
            let sessions = list_command_sessions_impl(&manager)
                .into_iter()
                .filter(|session| project_id.is_none_or(|id| session.project_id == id))
                .collect::<Vec<_>>();
            Ok(json!({ "sessions": sessions }))
        }
        TerminalIpcRequest::Output {
            id,
            project_directory,
        } => {
            require_session(app, &manager, id, project_directory.as_deref())?;
            let output = terminal::get_command_session_output_impl(&manager, id)?;
            Ok(json!({ "id": id, "output": clean_terminal_output(&output) }))
        }
        TerminalIpcRequest::Input {
            id,
            project_directory,
            text,
            enter,
        } => {
            let session =
                require_controllable_session(app, &manager, id, project_directory.as_deref())?;
            if session.status != crate::models::TerminalSessionStatus::Running {
                return Ok(json!({
                    "id": id,
                    "written": false,
                    "reason": "not_running",
                    "session": session,
                }));
            }
            if text.is_empty() && !enter {
                return Err(AppError::coded(
                    ErrorCode::CommandContentRequired,
                    "input text",
                ));
            }
            let data = if enter { format!("{text}\n") } else { text };
            write_command_session_impl(&manager, id, &data)?;
            Ok(json!({ "id": id, "written": true, "enter": enter }))
        }
        TerminalIpcRequest::Create {
            project_directory,
            command,
        } => {
            let session = create_registered_session(app, &manager, &project_directory, command)?;
            Ok(json!({ "started": true, "session": session }))
        }
        TerminalIpcRequest::Start {
            id,
            project_directory,
        } => {
            let session =
                require_controllable_session(app, &manager, id, project_directory.as_deref())?;
            if session.status == crate::models::TerminalSessionStatus::Running {
                return Ok(json!({ "started": false, "alreadyRunning": true, "session": session }));
            }
            let session = terminal::restart_command_session_impl(app, &manager, id)?;
            Ok(json!({ "started": true, "alreadyRunning": false, "session": session }))
        }
        TerminalIpcRequest::Stop {
            id,
            project_directory,
        } => {
            let session = require_session(app, &manager, id, project_directory.as_deref())?;
            let stop_requested = session.status == crate::models::TerminalSessionStatus::Running;
            if stop_requested {
                stop_command_session_impl(&manager, id)?;
            }
            Ok(json!({ "id": id, "stopRequested": stop_requested }))
        }
        TerminalIpcRequest::Restart {
            id,
            project_directory,
        } => {
            require_controllable_session(app, &manager, id, project_directory.as_deref())?;
            let session = terminal::restart_command_session_impl(app, &manager, id)?;
            Ok(json!({ "started": true, "session": session }))
        }
    }
}

fn resolve_project(app: &AppHandle, directory: &str) -> AppResult<(i64, String, String)> {
    use rusqlite::OptionalExtension;

    let directory = clean_str(directory);
    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    conn.query_row(
        "SELECT id, name, directory FROM projects WHERE directory = ?1 AND archived = 0",
        [&directory],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )
    .optional()?
    .ok_or_else(|| AppError::coded(ErrorCode::ProjectNotFound, directory))
}

fn require_session(
    app: &AppHandle,
    manager: &Arc<TerminalManager>,
    id: u64,
    project_directory: Option<&str>,
) -> AppResult<TerminalSessionInfo> {
    let project_id = project_directory
        .map(|directory| resolve_project(app, directory).map(|project| project.0))
        .transpose()?;
    list_command_sessions_impl(manager)
        .into_iter()
        .find(|session| {
            session.id == id && project_id.is_none_or(|project_id| session.project_id == project_id)
        })
        .ok_or_else(|| AppError::coded(ErrorCode::TerminalSessionNotFound, id.to_string()))
}

fn require_controllable_session(
    app: &AppHandle,
    manager: &Arc<TerminalManager>,
    id: u64,
    project_directory: Option<&str>,
) -> AppResult<TerminalSessionInfo> {
    let session = require_session(app, manager, id, project_directory)?;
    if terminal_kind_cli_controllable(&session.kind) {
        Ok(session)
    } else {
        Err(AppError::coded(
            ErrorCode::CommandNotFound,
            format!(
                "terminal session {id} kind {} is not CLI-controllable",
                session.kind
            ),
        ))
    }
}

fn terminal_kind_cli_controllable(kind: &str) -> bool {
    matches!(kind, "npm" | "custom")
}

fn command_not_found(detail: impl Into<String>) -> AppError {
    AppError::coded(ErrorCode::CommandNotFound, detail)
}

/// CLI 只能引用项目已有命令;最终命令文本统一由桌面端解析生成。
fn create_registered_session(
    app: &AppHandle,
    manager: &Arc<TerminalManager>,
    project_directory: &str,
    command: TerminalCommandRef,
) -> AppResult<TerminalSessionInfo> {
    let (project_id, project_name, project_path) = resolve_project(app, project_directory)?;
    match command {
        TerminalCommandRef::Npm { package, script } => {
            let group = crate::commands::script::package_scripts(&project_path)?
                .into_iter()
                .find(|group| group.dir == package)
                .ok_or_else(|| command_not_found(format!("npm package {package}")))?;
            let script = group
                .scripts
                .iter()
                .find(|item| item.name == script)
                .cloned()
                .ok_or_else(|| command_not_found(format!("npm script {package}:{script}")))?;
            let cwd = (group.dir != ".").then(|| {
                Path::new(&project_path)
                    .join(&group.dir)
                    .to_string_lossy()
                    .into_owned()
            });
            run_command_session_impl(
                app,
                manager,
                project_id,
                &project_name,
                &project_path,
                &format!("npm run {}", script.name),
                cwd.as_deref(),
                None,
                Some("npm"),
                Some(&script.name),
            )
        }
        TerminalCommandRef::Custom { id } => {
            let custom = {
                let db = app.state::<Db>();
                let conn = db.0.lock().unwrap();
                crate::commands::script::list_commands(&conn, project_id)?
                    .into_iter()
                    .find(|item| item.id == id)
            }
            .ok_or_else(|| command_not_found(format!("custom command {id}")))?;
            run_command_session_impl(
                app,
                manager,
                project_id,
                &project_name,
                &project_path,
                &custom.command,
                None,
                None,
                Some("custom"),
                Some(&custom.name),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_uses_stable_action_and_camel_case_fields() {
        let request = TerminalIpcRequest::Create {
            project_directory: "D:/repo".into(),
            command: TerminalCommandRef::Npm {
                package: "apps/web".into(),
                script: "dev".into(),
            },
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            json!({
                "action": "create",
                "projectDirectory": "D:/repo",
                "command": {
                    "kind": "npm",
                    "package": "apps/web",
                    "script": "dev",
                },
            })
        );
        let decoded: TerminalIpcRequest = serde_json::from_value(value).unwrap();
        assert!(matches!(decoded, TerminalIpcRequest::Create { .. }));

        let input = serde_json::to_value(TerminalIpcRequest::Input {
            id: 7,
            project_directory: Some("D:/repo".into()),
            text: "y".into(),
            enter: true,
        })
        .unwrap();
        assert_eq!(input["action"], json!("input"));
        assert_eq!(input["projectDirectory"], json!("D:/repo"));
        assert_eq!(input["enter"], json!(true));
        let decoded: TerminalIpcRequest = serde_json::from_value(input).unwrap();
        assert!(matches!(decoded, TerminalIpcRequest::Input { id: 7, .. }));
    }

    #[test]
    fn only_registered_command_tab_types_are_cli_controllable() {
        for kind in ["npm", "custom"] {
            assert!(terminal_kind_cli_controllable(kind));
        }
        for kind in ["shell", "ai", "docker", "java", "unknown"] {
            assert!(!terminal_kind_cli_controllable(kind));
        }
    }

    #[test]
    fn response_omits_unused_fields() {
        let success =
            serde_json::to_value(TerminalIpcResponse::success(json!({ "id": 1 }))).unwrap();
        assert_eq!(success, json!({ "ok": true, "result": { "id": 1 } }));
        let failure = serde_json::to_value(TerminalIpcResponse::error(
            "terminal_session_not_found",
            "终端 Tab 操作失败",
            Some("1".into()),
        ))
        .unwrap();
        assert_eq!(failure["ok"], json!(false));
        assert_eq!(failure["code"], json!("terminal_session_not_found"));
    }
}
