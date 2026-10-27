use super::*;

/// 删除目录(带重试)。取消克隆时 Windows 上被杀掉的子进程可能短暂持有
/// 文件句柄,立即 remove_dir_all 会失败,故重试几次
pub(super) async fn remove_dir_all_retry(path: &Path) -> std::io::Result<()> {
    for attempt in 0..5 {
        match tokio::fs::remove_dir_all(path).await {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt == 4 => return Err(e),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
    Ok(())
}

/// HTTP(S) URL 的查询、片段和 userinfo 均可能含令牌，会被 Git 保存或回显。
pub(crate) fn validate_clone_url(url: &str) -> AppResult<()> {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if matches!(parsed.scheme(), "http" | "https")
            && (!parsed.username().is_empty() || parsed.password().is_some()
                || parsed.query().is_some() || parsed.fragment().is_some())
        {
            return Err(AppError::coded(ErrorCode::GitCloneUrlRequired, "HTTP URL must not contain credentials, query or fragment"));
        }
    }
    Ok(())
}

fn validate_clone_transport(url: &str, uses_account: bool) -> AppResult<()> {
    if uses_account && reqwest::Url::parse(url).ok().is_some_and(|u| u.scheme() == "http") {
        return Err(AppError::coded(ErrorCode::GitCloneFailed, "account authentication requires HTTPS"));
    }
    Ok(())
}

/// 构造仅匹配目标仓库的临时 HTTP Basic 配置。
fn clone_auth_header(url: &str, provider: &str, username: &str, token: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    let parsed = reqwest::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return None;
    }
    let user = match provider {
        "github" => "x-access-token",
        "gitlab" => "oauth2",
        "gitee" => username,
        _ => return None,
    };
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"));
    Some((format!("http.{url}.extraheader"), format!("Authorization: Basic {basic}")))
}

#[cfg(test)]
mod security_tests {
    use super::{clone_auth_header, validate_clone_transport, validate_clone_url};

    #[test]
    fn clone_auth_is_scoped_and_not_embedded_in_url() {
        let (key, value) = clone_auth_header("https://example.com/org/repo.git", "github", "user", "secret").unwrap();
        assert_eq!(key, "http.https://example.com/org/repo.git.extraheader");
        assert!(!key.contains("secret"));
        assert!(value.starts_with("Authorization: Basic "));
        assert!(clone_auth_header("git@example.com:org/repo.git", "github", "user", "secret").is_none());
        assert!(clone_auth_header("http://example.com/repo", "github", "user", "secret").is_none());
    }

    #[test]
    fn clone_url_rejects_possible_secrets() {
        for url in ["https://user:secret@example.com/repo", "https://example.com/repo?access_token=secret", "https://example.com/repo?x=%73ecret", "https://example.com/repo#secret"] {
            let error = validate_clone_url(url).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
        assert!(validate_clone_url("https://example.com/repo").is_ok());
        assert!(validate_clone_url("git@example.com:org/repo.git").is_ok());
        assert!(validate_clone_transport("http://example.com/repo", true).is_err());
        assert!(validate_clone_transport("http://example.com/repo", false).is_ok());
        assert!(validate_clone_transport("https://example.com/repo", true).is_ok());
    }
}

/// 克隆仓库到本地目录,返回克隆后的路径。
/// 期间可通过 cancel_git_clone(job_id) 中断;失败/取消都会清理半成品目录。
/// stderr 仅排空，不透传潜在敏感信息。
/// account_id 传入时通过临时 Git 配置提供认证，origin 始终使用不含凭据的 URL。
#[tauri::command]
pub async fn git_clone(
    db: State<'_, Db>,
    url: String,
    target_path: String,
    job_id: String,
    account_id: Option<i64>,
    shallow: Option<bool>,
) -> AppResult<String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::coded(ErrorCode::GitCloneUrlRequired, ""));
    }
    validate_clone_url(&url)?;
    validate_clone_transport(&url, account_id.is_some())?;
    let credentials = match account_id {
        Some(id) if id == account::GH_CLI_ACCOUNT_ID => Some(account::gh_cli_git_credentials().await?),
        Some(id) => {
            let conn = db.0.lock().unwrap();
            Some(account::get_credentials(&conn, id)?)
        }
        None => None,
    };
    let auth = credentials.as_ref().and_then(|(provider, username, token)|
        clone_auth_header(&url, provider, username, token));
    let target = Path::new(&target_path);
    let parent = target
        .parent()
        .ok_or_else(|| AppError::coded(ErrorCode::GitCloneInvalidTarget, target_path.clone()))?;
    if !parent.is_dir() {
        return Err(AppError::coded(
            ErrorCode::GitCloneParentMissing,
            parent.display().to_string(),
        ));
    }
    if target.exists() {
        return Err(AppError::coded(
            ErrorCode::GitCloneTargetExists,
            target_path.clone(),
        ));
    }

    let mut command = tokio::process::Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    // Git 的临时配置经环境变量传递，不出现在 argv 或仓库配置中。
    // 按仓库 URL 限定 header，避免向其他主机发送凭据。
    if let Some((key, value)) = &auth {
        command.env("GIT_ALLOW_PROTOCOL", "https")
            .env("GIT_CONFIG_COUNT", "3")
            .env("GIT_CONFIG_KEY_0", "credential.helper")
            .env("GIT_CONFIG_VALUE_0", "")
            .env("GIT_CONFIG_KEY_1", key)
            .env("GIT_CONFIG_VALUE_1", value)
            .env("GIT_CONFIG_KEY_2", "http.followRedirects")
            .env("GIT_CONFIG_VALUE_2", "false");
    }
    command.arg("clone");
    if shallow.unwrap_or(false) {
        command.args(["--depth", "1"]);
    }
    command
        .args(["--", &url, &target_path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| AppError::coded(ErrorCode::GitCloneSpawnFailed, e.to_string()))?;
    // 登记 PID 供应用退出钩子按 PID 清理(child 随后 move 进 CLONE_JOBS,
    // 但 pid 已拷出为独立副本,不受句柄所有权转移影响)
    let _tracked = TrackedPid::new(child.id());

    // 持续排空 stderr，但不保存或展示其中可能回显的秘密。
    if let Some(mut stderr) = child.stderr.take() {
        tauri::async_runtime::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut chunk = [0u8; 4096];
            while matches!(stderr.read(&mut chunk).await, Ok(n) if n > 0) {}
        });
    }

    clone_jobs().lock().await.insert(job_id.clone(), child);

    // 轮询等待结束;注册表项被 cancel_git_clone 移除即视为用户取消
    let result: AppResult<()> = loop {
        let polled = {
            let mut jobs = clone_jobs().lock().await;
            match jobs.get_mut(&job_id) {
                None => break Err(AppError::coded(ErrorCode::GitCloneCanceled, "")),
                Some(child) => child.try_wait(),
            }
        };
        match polled {
            Ok(Some(status)) if status.success() => {
                clone_jobs().lock().await.remove(&job_id);
                break Ok(());
            }
            Ok(Some(_)) => {
                clone_jobs().lock().await.remove(&job_id);
                // stderr 可能包含重定向地址、服务端回显或凭据助手输出，不能安全透传。
                break Err(AppError::coded(ErrorCode::GitCloneFailed, ""));
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            Err(e) => {
                clone_jobs().lock().await.remove(&job_id);
                break Err(AppError::coded(
                    ErrorCode::GitClonePollFailed,
                    e.to_string(),
                ));
            }
        }
    };

    // 失败/取消时清理半成品目录(取消场景子进程刚被 kill,句柄释放有延迟,靠重试覆盖)
    if result.is_err() && target.exists() {
        let _ = remove_dir_all_retry(target).await;
    }
    result.map(|()| target_path)
}

/// 列出所有未归档项目的 origin 地址(非仓库/无 remote 的项目跳过),
/// 供前端与账号仓库列表做「已添加」匹配
#[tauri::command]
pub async fn list_project_remote_urls(db: State<'_, Db>) -> AppResult<Vec<String>> {
    let paths = {
        let conn = db.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT path FROM projects WHERE archived_at IS NULL")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    run_blocking(move || {
        let mut urls = Vec::new();
        for path in paths {
            // 非仓库/无 origin 的项目跳过
            let url = open_repo(&path).ok().flatten().and_then(|r| {
                r.find_remote("origin")
                    .ok()
                    .and_then(|remote| remote.url().ok().map(String::from))
            });
            if let Some(url) = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) {
                urls.push(url);
            }
        }
        Ok(urls)
    })
    .await
}

/// 取消进行中的克隆:kill 子进程并从注册表移除(git_clone 轮询发现后清理目录)。
/// Windows 上用 taskkill /T 杀整棵进程树(clone 会派生 remote helper 孙进程)
#[tauri::command]
pub async fn cancel_git_clone(job_id: String) -> AppResult<()> {
    let child = clone_jobs().lock().await.remove(&job_id);
    if let Some(mut child) = child {
        #[cfg(windows)]
        if let Some(pid) = child.id() {
            let mut cmd = std::process::Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let _ = cmd.output();
        }
        // 非 Windows 主路径;Windows 上作为 taskkill 的兜底(重复 kill 无害)
        let _ = child.start_kill();
    }
    Ok(())
}
