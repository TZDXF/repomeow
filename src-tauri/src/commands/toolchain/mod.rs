//! 设置页「工具链」面板:常用开发 CLI 的检测与安装/更新/卸载/版本切换。

mod detect;
mod dotnet;
mod git;
mod node;
mod operation;
mod process;
mod python;
mod remote;
mod rust;
mod version;

use crate::commands::open::{spawn_terminal, ShellKind};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{ToolchainRemoteVersion, ToolchainStatus};

use detect::{detect_toolchains_blocking, TOOLS};
use operation::{resolve_op, unsupported};
use process::{classify_source, cli_hits_on_path, display_path, user_home_path};
use remote::list_toolchain_versions_blocking;

#[tauri::command]
pub async fn detect_toolchains() -> AppResult<Vec<ToolchainStatus>> {
    tokio::task::spawn_blocking(detect_toolchains_blocking)
        .await
        .map_err(|error| AppError::coded(ErrorCode::IoError, error.to_string()))
}

#[tauri::command]
pub async fn list_toolchain_versions(tool: String) -> AppResult<Vec<ToolchainRemoteVersion>> {
    tokio::task::spawn_blocking(move || list_toolchain_versions_blocking(&tool))
        .await
        .map_err(|error| AppError::coded(ErrorCode::IoError, error.to_string()))?
}

#[tauri::command]
pub fn toolchain_op(tool: String, op: String, version: Option<String>) -> AppResult<()> {
    let tool = tool.trim();
    let op = op.trim();
    if TOOLS.iter().any(|spec| spec.id == tool) {
        let source = cli_hits_on_path(tool)
            .into_iter()
            .next()
            .as_ref()
            .map(|path| classify_source(path));
        let command = resolve_op(tool, op, version.as_deref(), source.as_deref())?;
        let home = user_home_path()
            .map(|path| display_path(&path))
            .unwrap_or_else(|| ".".to_string());
        // 工具链命令文本是 cmd 专属语法(del /f、%USERPROFILE%、`&` 串联),
        // 在 PowerShell / bash 下会直接报错,固定走 cmd,不跟随终端设置
        return spawn_terminal(
            &home,
            &format!("Toolchain: {tool}"),
            Some(&command),
            ShellKind::Cmd,
        );
    }
    Err(unsupported(tool, op))
}

#[cfg(test)]
mod tests;
