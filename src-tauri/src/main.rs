// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if repomeow_lib::mcp::is_mcp_mode() {
        if let Err(error) = repomeow_lib::mcp::serve_stdio_blocking() {
            eprintln!("RepoMeow MCP 服务退出：{error}");
            std::process::exit(1);
        }
        return;
    }
    repomeow_lib::run()
}
