use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};

/// 代码托管平台账号(GitHub / Gitee / 自建 GitLab)。
/// token 以明文落库且不回传,前端仅能看到 token_preview 脱敏预览。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitAccount {
    pub id: i64,
    pub provider: String,
    pub label: String,
    pub base_url: String,
    pub username: String,
    pub token_preview: String,
    /// 拉取仓库遇到 401 时由后端置 true,前端设置页据此标记「Token 已失效」
    pub token_invalid: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 平台账号下的远程仓库(各平台 API 字段归一化后的结构)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    pub repo_id: String,
    /// 所属组织/用户名(namespace)
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub description: String,
    pub html_url: String,
    pub http_clone_url: String,
    pub ssh_clone_url: String,
    pub default_branch: String,
    pub is_private: bool,
    pub updated_at: String,
}

fn now() -> i64 {
    crate::time_util::now_ts()
}

fn normalize_provider(provider: &str) -> AppResult<String> {
    let p = provider.trim().to_lowercase();
    match p.as_str() {
        "github" | "gitee" | "gitlab" => Ok(p),
        _ => Err(AppError::coded(ErrorCode::AccountUnsupportedProvider, provider.to_string())),
    }
}

/// github/gitee 用固定地址;gitlab 取用户填写的实例地址(支持 http 内网地址)
fn resolve_base_url(provider: &str, input: Option<&str>) -> AppResult<String> {
    match provider {
        "github" => Ok("https://github.com".to_string()),
        "gitee" => Ok("https://gitee.com".to_string()),
        "gitlab" => {
            let raw = input.unwrap_or("").trim().trim_end_matches('/');
            if raw.is_empty() {
                return Err(AppError::coded(ErrorCode::GitlabBaseUrlRequired, ""));
            }
            if !raw.starts_with("http://") && !raw.starts_with("https://") {
                return Err(AppError::coded(
                    ErrorCode::GitlabBaseUrlInvalidScheme,
                    raw.to_string(),
                ));
            }
            Ok(raw.to_string())
        }
        _ => Err(AppError::coded(ErrorCode::AccountUnsupportedProvider, provider.to_string())),
    }
}

fn api_base(provider: &str, base_url: &str) -> String {
    match provider {
        "github" => "https://api.github.com".to_string(),
        "gitee" => format!("{base_url}/api/v5"),
        "gitlab" => format!("{base_url}/api/v4"),
        _ => base_url.to_string(),
    }
}

/// 脱敏预览:只保留末 4 位
fn token_preview(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() >= 4 {
        format!(
            "****{}",
            chars[chars.len() - 4..].iter().collect::<String>()
        )
    } else {
        "****".to_string()
    }
}

struct AccountRow {
    id: i64,
    provider: String,
    label: String,
    base_url: String,
    username: String,
    token: String,
    token_invalid: bool,
    created_at: i64,
    updated_at: i64,
}

const ACCOUNT_COLS: &str =
    "id, provider, label, base_url, username, token, token_invalid, created_at, updated_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    Ok(AccountRow {
        id: r.get(0)?,
        provider: r.get(1)?,
        label: r.get(2)?,
        base_url: r.get(3)?,
        username: r.get(4)?,
        token: r.get(5)?,
        token_invalid: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn row_to_account(row: &AccountRow) -> GitAccount {
    GitAccount {
        id: row.id,
        provider: row.provider.clone(),
        label: row.label.clone(),
        base_url: row.base_url.clone(),
        username: row.username.clone(),
        token_preview: token_preview(&row.token),
        token_invalid: row.token_invalid,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn get_account_row(conn: &Connection, id: i64) -> AppResult<AccountRow> {
    let sql = format!("SELECT {ACCOUNT_COLS} FROM git_accounts WHERE id = ?1");
    conn.query_row(&sql, params![id], map_row)
        .optional()?
        .ok_or_else(|| AppError::coded(ErrorCode::AccountNotFound, id.to_string()))
}

/// 供 git_clone 使用:取账号的 (provider, username, token)
pub(crate) fn get_credentials(conn: &Connection, id: i64) -> AppResult<(String, String, String)> {
    let row = get_account_row(conn, id)?;
    Ok((row.provider, row.username, row.token))
}

/// 把账号凭据拼进 http(s) clone URL(克隆成功后应重置 remote 为干净 URL,避免 token 残留 .git/config)
pub(crate) fn build_authed_url(provider: &str, username: &str, token: &str, url: &str) -> String {
    let userinfo = match provider {
        "github" => format!("x-access-token:{token}"),
        "gitlab" => format!("oauth2:{token}"),
        "gitee" => format!("{username}:{token}"),
        _ => return url.to_string(),
    };
    if let Some(rest) = url.strip_prefix("https://") {
        format!("https://{userinfo}@{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("http://{userinfo}@{rest}")
    } else {
        url.to_string()
    }
}

// ── GitHub CLI(gh)虚拟账号 ─────────────────────────────────
// 不落库、不出现在设置页账号列表;前端在「账号仓库」下拉中并入,
// 仓库列表与克隆都复用 github REST/token 链路,token 取自 `gh auth token`。

/// 虚拟账号 id(DB 自增 id 从 1 开始,0 不会与真实账号冲突)
pub const GH_CLI_ACCOUNT_ID: i64 = 0;

fn gh_command() -> std::process::Command {
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

fn run_gh(args: &[&str]) -> AppResult<String> {
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
fn gh_cli_credentials() -> AppResult<(String, String)> {
    let username = run_gh(&["api", "user", "--jq", ".login"])?;
    let token = run_gh(&["auth", "token"])?;
    if username.is_empty() || token.is_empty() {
        return Err(AppError::coded(ErrorCode::GhCliIncompleteCredentials, ""));
    }
    Ok((username, token))
}

/// 用 gh 凭据合成一行 github 账号,供 REST 拉取链路复用
fn gh_cli_account_row() -> AppResult<AccountRow> {
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

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("repomeow")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 鉴权方式:GitHub/GitLab 走请求头,Gitee 走 access_token 查询参数(在 URL 里拼)
fn apply_auth(
    req: reqwest::RequestBuilder,
    provider: &str,
    token: &str,
) -> reqwest::RequestBuilder {
    match provider {
        "github" => req
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json"),
        "gitlab" => req.header("PRIVATE-TOKEN", token),
        _ => req,
    }
}

async fn send(req: reqwest::RequestBuilder) -> AppResult<reqwest::Response> {
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::coded(ErrorCode::PlatformConnectionFailed, e.to_string()))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let body = body.trim();
    // 401 用结构化错误码:前端按 code 本地化,list_account_repos 也据此给账号打失效标记
    if status.as_u16() == 401 {
        return Err(AppError::Coded {
            code: ErrorCode::AccountTokenInvalid,
            message: String::new(),
        });
    }
    // 已知状态码直接给友好文案,不把平台返回的原始 JSON 拼给用户
    match status.as_u16() {
        403 => return Err(AppError::coded(ErrorCode::PlatformForbidden, "")),
        404 => return Err(AppError::coded(ErrorCode::PlatformNotFound, "")),
        _ => {}
    }
    // 其他状态码:尝试提取响应 JSON 的 message 字段,避免把整段原始响应丢给用户
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| body.chars().take(200).collect());
    if detail.is_empty() {
        Err(AppError::coded(
            ErrorCode::PlatformRequestFailed,
            format!("status={status}"),
        ))
    } else {
        Err(AppError::coded(
            ErrorCode::PlatformRequestFailedWithDetail,
            format!("status={status} detail={detail}"),
        ))
    }
}

/// 调用平台 /user 端点验证 token 并取用户名
async fn fetch_username(provider: &str, base_url: &str, token: &str) -> AppResult<String> {
    let api = api_base(provider, base_url);
    let url = match provider {
        "gitee" => format!("{api}/user?access_token={token}"),
        _ => format!("{api}/user"),
    };
    let resp = send(apply_auth(http_client().get(&url), provider, token)).await?;
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::coded(ErrorCode::UserInfoParseFailed, e.to_string()))?;
    let key = if provider == "gitlab" {
        "username"
    } else {
        "login"
    };
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::coded(ErrorCode::UserInfoMissingUsername, ""))
}

fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// 取嵌套字段(如 v["owner"]["login"])
fn json_nested_str(v: &serde_json::Value, path: &[&str]) -> String {
    let mut cur = v;
    for key in path {
        let Some(next) = cur.get(key) else {
            return String::new();
        };
        cur = next;
    }
    cur.as_str().unwrap_or("").to_string()
}

/// owner 兜底:从 full_name 去掉末段仓库名
fn owner_from_full_name(full_name: &str) -> String {
    full_name
        .rsplit_once('/')
        .map(|(o, _)| o.to_string())
        .unwrap_or_default()
}

fn parse_repos(provider: &str, items: &[serde_json::Value]) -> Vec<RemoteRepo> {
    items
        .iter()
        .map(|v| {
            let (
                repo_id,
                owner,
                name,
                full_name,
                html_url,
                http_clone_url,
                ssh_clone_url,
                updated_at,
                is_private,
            ) = match provider {
                "github" => (
                    v.get("id").map(|x| x.to_string()).unwrap_or_default(),
                    json_nested_str(v, &["owner", "login"]),
                    json_str(v, "name"),
                    json_str(v, "full_name"),
                    json_str(v, "html_url"),
                    json_str(v, "clone_url"),
                    json_str(v, "ssh_url"),
                    json_str(v, "updated_at"),
                    v.get("private").and_then(|x| x.as_bool()).unwrap_or(false),
                ),
                "gitee" => {
                    let html = json_str(v, "html_url");
                    let http = if html.is_empty() {
                        String::new()
                    } else {
                        format!("{html}.git")
                    };
                    (
                        v.get("id").map(|x| x.to_string()).unwrap_or_default(),
                        json_nested_str(v, &["namespace", "path"]),
                        json_str(v, "name"),
                        json_str(v, "full_name"),
                        html,
                        http,
                        json_str(v, "ssh_url"),
                        json_str(v, "updated_at"),
                        v.get("private").and_then(|x| x.as_bool()).unwrap_or(false),
                    )
                }
                // gitlab
                _ => (
                    v.get("id").map(|x| x.to_string()).unwrap_or_default(),
                    json_nested_str(v, &["namespace", "full_path"]),
                    json_str(v, "name"),
                    json_str(v, "path_with_namespace"),
                    json_str(v, "web_url"),
                    json_str(v, "http_url_to_repo"),
                    json_str(v, "ssh_url_to_repo"),
                    json_str(v, "last_activity_at"),
                    json_str(v, "visibility") != "public",
                ),
            };
            let owner = if owner.is_empty() {
                owner_from_full_name(&full_name)
            } else {
                owner
            };
            RemoteRepo {
                repo_id,
                owner,
                name,
                full_name,
                description: json_str(v, "description"),
                html_url,
                http_clone_url,
                ssh_clone_url,
                default_branch: json_str(v, "default_branch"),
                is_private,
                updated_at,
            }
        })
        .collect()
}

/// 发送 GET 并解析为 JSON 数组
async fn fetch_json_array(
    url: &str,
    provider: &str,
    token: &str,
) -> AppResult<Vec<serde_json::Value>> {
    let resp = send(apply_auth(http_client().get(url), provider, token)).await?;
    resp.json()
        .await
        .map_err(|e| AppError::coded(ErrorCode::RepoListParseFailed, e.to_string()))
}

/// 拉取单页仓库列表
async fn fetch_repos_page(
    row: &AccountRow,
    page: u32,
    per_page: u32,
) -> AppResult<Vec<RemoteRepo>> {
    let api = api_base(&row.provider, &row.base_url);
    let url = match row.provider.as_str() {
        "github" => format!(
            "{api}/user/repos?affiliation=owner,collaborator,organization_member&sort=updated&direction=desc&page={page}&per_page={per_page}"
        ),
        "gitee" => format!(
            "{api}/user/repos?access_token={}&sort=updated&direction=desc&page={page}&per_page={per_page}",
            row.token
        ),
        // gitlab
        _ => format!(
            "{api}/projects?membership=true&order_by=updated_at&sort=desc&page={page}&per_page={per_page}"
        ),
    };
    let items = fetch_json_array(&url, &row.provider, &row.token).await?;
    Ok(parse_repos(&row.provider, &items))
}

/// Gitee: 列出 token 可访问的组织(/user/repos 只含个人仓库,组织仓库需按组织单独拉)
async fn fetch_gitee_orgs(row: &AccountRow) -> AppResult<Vec<String>> {
    let api = api_base("gitee", &row.base_url);
    let mut orgs = Vec::new();
    for page in 1..=10u32 {
        let url = format!(
            "{api}/user/orgs?access_token={}&page={page}&per_page=100",
            row.token
        );
        let items = fetch_json_array(&url, "gitee", &row.token).await?;
        let short = items.len() < 100;
        orgs.extend(
            items
                .iter()
                .map(|v| json_str(v, "login"))
                .filter(|s| !s.is_empty()),
        );
        if short {
            break;
        }
    }
    Ok(orgs)
}

/// Gitee: 拉取某组织下单页仓库
async fn fetch_gitee_org_repos_page(
    row: &AccountRow,
    org: &str,
    page: u32,
    per_page: u32,
) -> AppResult<Vec<RemoteRepo>> {
    let api = api_base("gitee", &row.base_url);
    let url = format!(
        "{api}/orgs/{org}/repos?access_token={}&page={page}&per_page={per_page}",
        row.token
    );
    let items = fetch_json_array(&url, "gitee", &row.token).await?;
    Ok(parse_repos("gitee", &items))
}

#[tauri::command]
pub fn list_git_accounts(db: State<'_, Db>) -> AppResult<Vec<GitAccount>> {
    let conn = db.0.lock().unwrap();
    let sql = format!("SELECT {ACCOUNT_COLS} FROM git_accounts ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    Ok(rows
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(row_to_account)
        .collect())
}

/// 绑定账号:先调平台 API 验证 token 并取 username,成功才落库
#[tauri::command]
pub async fn add_git_account(
    db: State<'_, Db>,
    provider: String,
    label: String,
    base_url: Option<String>,
    token: String,
) -> AppResult<GitAccount> {
    let provider = normalize_provider(&provider)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::coded(ErrorCode::AccountTokenRequired, ""));
    }
    let base = resolve_base_url(&provider, base_url.as_deref())?;
    let username = fetch_username(&provider, &base, &token).await?;

    let conn = db.0.lock().unwrap();
    let ts = now();
    conn.execute(
        "INSERT INTO git_accounts (provider, label, base_url, username, token, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![provider, label.trim(), base, username, token, ts, ts],
    )?;
    let row = get_account_row(&conn, conn.last_insert_rowid())?;
    Ok(row_to_account(&row))
}

/// 更新账号;token 传空表示保留原 token。token 或实例地址变化时重新调 API 验证并刷新 username
#[tauri::command]
pub async fn update_git_account(
    db: State<'_, Db>,
    id: i64,
    label: String,
    base_url: Option<String>,
    token: Option<String>,
) -> AppResult<GitAccount> {
    let existing = {
        let conn = db.0.lock().unwrap();
        get_account_row(&conn, id)?
    };
    let new_token = token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let base = if existing.provider == "gitlab" {
        resolve_base_url(
            "gitlab",
            base_url.as_deref().or(Some(existing.base_url.as_str())),
        )?
    } else {
        existing.base_url.clone()
    };

    let mut username = existing.username.clone();
    // token 或实例地址变化时重新调 API 验证;验证通过即清除失效标记
    let mut verified = false;
    if new_token.is_some() || base != existing.base_url {
        let token_to_use = new_token.clone().unwrap_or_else(|| existing.token.clone());
        username = fetch_username(&existing.provider, &base, &token_to_use).await?;
        verified = true;
    }

    let conn = db.0.lock().unwrap();
    conn.execute(
        "UPDATE git_accounts
         SET label = ?1, base_url = ?2, username = ?3,
             token = COALESCE(?4, token), updated_at = ?5,
             token_invalid = CASE WHEN ?7 THEN 0 ELSE token_invalid END
         WHERE id = ?6",
        params![label.trim(), base, username, new_token, now(), id, verified],
    )?;
    let row = get_account_row(&conn, id)?;
    Ok(row_to_account(&row))
}

#[tauri::command]
pub fn remove_git_account(db: State<'_, Db>, id: i64) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    let affected = conn.execute("DELETE FROM git_accounts WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(AppError::coded(ErrorCode::AccountNotFound, id.to_string()));
    }
    Ok(())
}

/// 循环分页拉取账号下全部仓库(每页 100,上限 1000 条防失控)
async fn fetch_all_repos(row: &AccountRow) -> AppResult<Vec<RemoteRepo>> {
    const PER_PAGE: u32 = 100;
    const MAX_PAGES: u32 = 10;
    let mut all = Vec::new();
    for page in 1..=MAX_PAGES {
        let items = fetch_repos_page(row, page, PER_PAGE).await?;
        let short = items.len() < PER_PAGE as usize;
        all.extend(items);
        if short {
            break;
        }
    }
    // Gitee 的 /user/repos 只含个人仓库,组织仓库需按组织逐个拉取并按 full_name 去重;
    // 组织接口失败不阻断已拿到的个人仓库
    if row.provider == "gitee" {
        let orgs = fetch_gitee_orgs(row).await.unwrap_or_default();
        let mut seen: std::collections::HashSet<String> = all
            .iter()
            .map(|r: &RemoteRepo| r.full_name.to_lowercase())
            .collect();
        for org in orgs {
            for page in 1..=MAX_PAGES {
                let repos = fetch_gitee_org_repos_page(row, &org, page, PER_PAGE).await?;
                let short = repos.len() < PER_PAGE as usize;
                for r in repos {
                    if seen.insert(r.full_name.to_lowercase()) {
                        all.push(r);
                    }
                }
                if short {
                    break;
                }
            }
        }
    }
    // 统一按更新时间倒序(ISO 8601 字符串字典序近似时间序)
    all.sort_by(|a: &RemoteRepo, b: &RemoteRepo| b.updated_at.cmp(&a.updated_at));
    Ok(all)
}

/// 一次拉取账号下全部仓库(前端只做客户端搜索过滤)
#[tauri::command]
pub async fn list_account_repos(db: State<'_, Db>, account_id: i64) -> AppResult<Vec<RemoteRepo>> {
    // GitHub CLI 虚拟账号:不查库,token 取自 gh(进程调用放 blocking 线程)
    if account_id == GH_CLI_ACCOUNT_ID {
        let row = tokio::task::spawn_blocking(gh_cli_account_row)
            .await
            .map_err(|e| AppError::coded(ErrorCode::GhCliCredentialsFailed, e.to_string()))??;
        return fetch_all_repos(&row).await;
    }
    let row = {
        let conn = db.0.lock().unwrap();
        get_account_row(&conn, account_id)?
    };
    let result = fetch_all_repos(&row).await;
    // Token 失效(401)时落库标记,设置页账号列表据此提示用户更新 Token
    if let Err(e) = &result {
        if matches!(
            e,
            AppError::Coded {
                code: ErrorCode::AccountTokenInvalid,
                ..
            }
        ) {
            let conn = db.0.lock().unwrap();
            let _ = conn.execute(
                "UPDATE git_accounts SET token_invalid = 1 WHERE id = ?1",
                params![account_id],
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_preview_masks_all_but_last4() {
        assert_eq!(token_preview("ghp_abcdef123456"), "****3456");
        assert_eq!(token_preview("abc"), "****");
    }

    #[test]
    fn resolve_base_url_rules() {
        assert_eq!(
            resolve_base_url("github", None).unwrap(),
            "https://github.com"
        );
        assert_eq!(
            resolve_base_url("gitee", None).unwrap(),
            "https://gitee.com"
        );
        // gitlab: 去尾斜杠,允许 http 内网地址
        assert_eq!(
            resolve_base_url("gitlab", Some("https://gitlab.example.com/")).unwrap(),
            "https://gitlab.example.com"
        );
        assert_eq!(
            resolve_base_url("gitlab", Some("http://192.168.1.10:8080")).unwrap(),
            "http://192.168.1.10:8080"
        );
        assert!(resolve_base_url("gitlab", Some("")).is_err());
        assert!(resolve_base_url("gitlab", Some("gitlab.example.com")).is_err());
    }

    #[test]
    fn build_authed_url_embeds_credentials() {
        assert_eq!(
            build_authed_url("github", "octo", "tok", "https://github.com/a/b.git"),
            "https://x-access-token:tok@github.com/a/b.git"
        );
        assert_eq!(
            build_authed_url("gitlab", "u", "tok", "https://lab.local/a/b.git"),
            "https://oauth2:tok@lab.local/a/b.git"
        );
        assert_eq!(
            build_authed_url("gitee", "octo", "tok", "https://gitee.com/a/b.git"),
            "https://octo:tok@gitee.com/a/b.git"
        );
        // ssh 地址不处理
        assert_eq!(
            build_authed_url("github", "octo", "tok", "git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
    }

    #[test]
    fn db_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let ts = now();
        conn.execute(
            "INSERT INTO git_accounts (provider, label, base_url, username, token, created_at, updated_at)
             VALUES ('github', '工作', 'https://github.com', 'octo', 'ghp_secret1234', ?1, ?1)",
            params![ts],
        )
        .unwrap();
        let row = get_account_row(&conn, 1).unwrap();
        assert_eq!(row.username, "octo");
        let account = row_to_account(&row);
        assert_eq!(account.token_preview, "****1234");
        assert_eq!(account.provider, "github");
        // 迁移新增列默认未失效
        assert!(!account.token_invalid);

        let (provider, username, token) = get_credentials(&conn, 1).unwrap();
        assert_eq!(
            (provider.as_str(), username.as_str(), token.as_str()),
            ("github", "octo", "ghp_secret1234")
        );

        assert!(get_account_row(&conn, 999).is_err());
    }
}
