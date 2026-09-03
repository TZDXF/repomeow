
use crate::error::{AppError, AppResult, ErrorCode};
use super::*;

// ── GitHub CLI(gh)虚拟账号 ─────────────────────────────────
// 不落库、不出现在设置页账号列表;前端在「账号仓库」下拉中并入,
// 仓库列表与克隆都复用 github REST/token 链路,token 取自 `gh auth token`。

/// 虚拟账号 id(DB 自增 id 从 1 开始,0 不会与真实账号冲突)
pub const GH_CLI_ACCOUNT_ID: i64 = 0;

pub(super) fn gh_command() -> std::process::Command {
    let mut cmd = std::process::Command::new("gh");
    // GUI 应用无法应答交互式提示
    cmd.env("GH_PROMPT_DISABLED", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

pub(super) fn run_gh(args: &[&str]) -> AppResult<String> {
    let out = gh_command()
        .args(args)
        .output()
        .map_err(|e| AppError::coded(ErrorCode::GhCliSpawnFailed, e.to_string()))?;
    if !out.status.success() {
        return Err(AppError::coded(ErrorCode::GhCliNotFound, ""));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 探测 gh CLI:已安装且已登录则返回 (username, token)
pub(super) fn gh_cli_credentials() -> AppResult<(String, String)> {
    let username = run_gh(&["api", "user", "--jq", ".login"])?;
    let token = run_gh(&["auth", "token"])?;
    if username.is_empty() || token.is_empty() {
        return Err(AppError::coded(ErrorCode::GhCliIncompleteCredentials, ""));
    }
    Ok((username, token))
}

/// 用 gh 凭据合成一行 github 账号,供 REST 拉取链路复用
pub(super) fn gh_cli_account_row() -> AppResult<AccountRow> {
    let (username, token) = gh_cli_credentials()?;
    Ok(AccountRow {
        id: GH_CLI_ACCOUNT_ID,
        provider: "github".to_string(),
        label: "GitHub CLI".to_string(),
        base_url: "https://github.com".to_string(),
        username,
        token,
        token_invalid: false,
        created_at: 0,
        updated_at: 0,
    })
}

/// 探测 gh CLI 是否可用(已安装且已登录),可用时返回虚拟账号供前端并入账号下拉;
/// 不可用返回 Ok(None),由前端静默降级(下拉不显示该项)
#[tauri::command]
pub async fn get_gh_cli_account() -> AppResult<Option<GitAccount>> {
    tokio::task::spawn_blocking(|| match gh_cli_account_row() {
        Ok(row) => Ok(Some(row_to_account(&row))),
        Err(_) => Ok(None),
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::GhCliDetectFailed, e.to_string()))?
}

/// 供 git_clone 使用:取 gh CLI 的 (provider, username, token)
pub(crate) async fn gh_cli_git_credentials() -> AppResult<(String, String, String)> {
    tokio::task::spawn_blocking(|| {
        let (username, token) = gh_cli_credentials()?;
        Ok(("github".to_string(), username, token))
    })
    .await
    .map_err(|e| AppError::coded(ErrorCode::GhCliCredentialsFailed, e.to_string()))?
}

