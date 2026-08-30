use serde::Serialize;

use crate::error::AppResult;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub executable: String,
    pub args: Vec<String>,
}

/// 返回 MCP 客户端应启动的当前 RepoMeow 主程序及参数。
/// MCP 与桌面应用共用同一个可执行文件，不产生或分发独立 Sidecar。
#[tauri::command]
pub fn get_mcp_server_info() -> AppResult<McpServerInfo> {
    Ok(McpServerInfo {
        executable: std::env::current_exe()?.to_string_lossy().into_owned(),
        args: vec!["--mcp".to_string()],
    })
}
