
use crate::error::{AppError, AppResult, ErrorCode};
use super::*;

pub(super) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("repomeow")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 鉴权方式:GitHub/GitLab 走请求头,Gitee 走 access_token 查询参数(在 URL 里拼)
pub(super) fn apply_auth(
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

pub(super) async fn send(req: reqwest::RequestBuilder) -> AppResult<reqwest::Response> {
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
pub(super) async fn fetch_username(provider: &str, base_url: &str, token: &str) -> AppResult<String> {
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

pub(super) fn json_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// 取嵌套字段(如 v["owner"]["login"])
pub(super) fn json_nested_str(v: &serde_json::Value, path: &[&str]) -> String {
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
pub(super) fn owner_from_full_name(full_name: &str) -> String {
    full_name
        .rsplit_once('/')
        .map(|(o, _)| o.to_string())
        .unwrap_or_default()
}

pub(super) fn parse_repos(provider: &str, items: &[serde_json::Value]) -> Vec<RemoteRepo> {
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
pub(super) async fn fetch_json_array(
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
pub(super) async fn fetch_repos_page(
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
pub(super) async fn fetch_gitee_orgs(row: &AccountRow) -> AppResult<Vec<String>> {
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
pub(super) async fn fetch_gitee_org_repos_page(
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

