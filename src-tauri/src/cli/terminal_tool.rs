use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::util::{repomeow_data_root, ToolFailure};
use super::{TerminalCommands, TerminalCreateCommands};
use crate::terminal_ipc::{
    TerminalCommandRef, TerminalIpcRequest, TerminalIpcResponse, TERMINAL_IPC_ARG,
};

const DESKTOP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const RESPONSE_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(super) async fn execute(command: TerminalCommands) -> Result<Value, ToolFailure> {
    let request = match command {
        TerminalCommands::List { project_directory } => {
            TerminalIpcRequest::List { project_directory }
        }
        TerminalCommands::Output {
            id,
            project_directory,
        } => TerminalIpcRequest::Output {
            id,
            project_directory,
        },
        TerminalCommands::Input {
            id,
            project_directory,
            text,
            enter,
        } => TerminalIpcRequest::Input {
            id,
            project_directory,
            text,
            enter,
        },
        TerminalCommands::Create { command } => match command {
            TerminalCreateCommands::Npm {
                project_directory,
                name,
                package,
            } => TerminalIpcRequest::Create {
                project_directory,
                command: TerminalCommandRef::Npm {
                    package,
                    script: name,
                },
            },
            TerminalCreateCommands::Custom {
                project_directory,
                id,
            } => TerminalIpcRequest::Create {
                project_directory,
                command: TerminalCommandRef::Custom { id },
            },
        },
        TerminalCommands::Start {
            id,
            project_directory,
        } => TerminalIpcRequest::Start {
            id,
            project_directory,
        },
        TerminalCommands::Stop {
            id,
            project_directory,
        } => TerminalIpcRequest::Stop {
            id,
            project_directory,
        },
        TerminalCommands::Restart {
            id,
            project_directory,
        } => TerminalIpcRequest::Restart {
            id,
            project_directory,
        },
    };
    execute_via_desktop(request).await
}

async fn execute_via_desktop(request: TerminalIpcRequest) -> Result<Value, ToolFailure> {
    let (request_path, response_path) = create_request(&request)?;
    let result = launch_desktop_and_wait(&request_path, &response_path).await;
    let _ = std::fs::remove_file(&request_path);
    let _ = std::fs::remove_file(&response_path);
    result
}

fn create_request(request: &TerminalIpcRequest) -> Result<(PathBuf, PathBuf), ToolFailure> {
    let directory = repomeow_data_root()?.join("ipc");
    std::fs::create_dir_all(&directory).map_err(|error| {
        ToolFailure::new("terminal_ipc_prepare_failed", "创建终端控制目录失败")
            .with_detail(error.to_string())
    })?;
    let id = format!(
        "terminal-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    );
    let request_path = directory.join(format!("{id}.request.json"));
    let response_path = directory.join(format!("{id}.response.json"));
    let payload = serde_json::to_vec(request).map_err(|error| {
        ToolFailure::new("terminal_ipc_encode_failed", "编码终端控制请求失败")
            .with_detail(error.to_string())
    })?;
    std::fs::write(&request_path, payload).map_err(|error| {
        ToolFailure::new("terminal_ipc_write_failed", "写入终端控制请求失败")
            .with_detail(error.to_string())
    })?;
    Ok((request_path, response_path))
}

async fn launch_desktop_and_wait(
    request_path: &Path,
    response_path: &Path,
) -> Result<Value, ToolFailure> {
    let executable = std::env::current_exe().map_err(|error| {
        ToolFailure::new("desktop_launch_failed", "无法定位 RepoMeow 可执行文件")
            .with_detail(error.to_string())
    })?;
    Command::new(executable)
        .arg(TERMINAL_IPC_ARG)
        .arg(request_path)
        .arg(response_path)
        .spawn()
        .map_err(|error| {
            ToolFailure::new("desktop_launch_failed", "启动 RepoMeow 桌面应用失败")
                .with_detail(error.to_string())
        })?;

    let started = Instant::now();
    loop {
        if response_path.is_file() {
            let raw = std::fs::read_to_string(response_path).map_err(|error| {
                ToolFailure::new("terminal_ipc_read_failed", "读取桌面应用响应失败")
                    .with_detail(error.to_string())
            })?;
            let response: TerminalIpcResponse = serde_json::from_str(&raw).map_err(|error| {
                ToolFailure::new("terminal_ipc_invalid_response", "桌面应用响应格式无效")
                    .with_detail(error.to_string())
            })?;
            if response.ok {
                return Ok(response.result.unwrap_or(Value::Null));
            }
            return Err(ToolFailure {
                code: response
                    .code
                    .unwrap_or_else(|| "terminal_ipc_failed".to_string()),
                message: response
                    .message
                    .unwrap_or_else(|| "终端 Tab 操作失败".to_string()),
                detail: response.detail,
            });
        }
        if started.elapsed() >= DESKTOP_RESPONSE_TIMEOUT {
            return Err(ToolFailure::new(
                "desktop_response_timeout",
                "等待 RepoMeow 桌面应用响应超时",
            )
            .with_detail("请确认桌面应用已启动且版本支持终端 CLI 控制后重试"));
        }
        tokio::time::sleep(RESPONSE_POLL_INTERVAL).await;
    }
}
