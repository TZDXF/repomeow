#[tokio::main]
async fn main() {
    if let Err(error) = repomeow_lib::mcp::serve_stdio().await {
        eprintln!("RepoMeow MCP 服务退出：{error}");
        std::process::exit(1);
    }
}
