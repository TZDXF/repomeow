use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};

mod gh;
mod http;
#[cfg(test)]
mod tests;

pub use gh::*;
pub(crate) use gh::gh_cli_git_credentials;
use http::*;

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
        _ => Err(AppError::coded(
            ErrorCode::AccountUnsupportedProvider,
            provider.to_string(),
        )),
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
        _ => Err(AppError::coded(
            ErrorCode::AccountUnsupportedProvider,
            provider.to_string(),
        )),
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

pub(super) struct AccountRow {
    pub(super) id: i64,
    pub(super) provider: String,
    pub(super) label: String,
    pub(super) base_url: String,
    pub(super) username: String,
    pub(super) token: String,
    pub(super) token_invalid: bool,
    pub(super) created_at: i64,
    pub(super) updated_at: i64,
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

