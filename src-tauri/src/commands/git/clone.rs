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

/// 克隆仓库到本地目录,返回克隆后的路径。
/// 期间可通过 cancel_git_clone(job_id) 中断;失败/取消都会清理半成品目录。
/// 进度行刷在 stderr 但不透传(前端仅 loading),只保留末尾用于错误提示。
/// account_id 传入时(从「账号仓库」入口克隆)用绑定账号的 token 拼认证 URL 克隆,
/// 成功后把 origin 重置为干净 URL,避免 token 残留在 .git/config
#[tauri::command]
pub async fn git_clone(
    db: State<'_, Db>,
    url: String,
    target_path: String,
    job_id: String,
    account_id: Option<i64>,
) -> AppResult<String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::coded(ErrorCode::GitCloneUrlRequired, ""));
    }
    // 账号凭据拼进 clone URL(仅 http(s) 地址生效,ssh 地址原样使用)
    let clone_url = match account_id {
        // GitHub CLI 虚拟账号:不查库,token 取自 gh(必须先于查库分支)
        Some(id) if id == account::GH_CLI_ACCOUNT_ID => {
            let (provider, username, token) = account::gh_cli_git_credentials().await?;
            account::build_authed_url(&provider, &username, &token, &url)
        }
        Some(id) => {
            let (provider, username, token) = {
                let conn = db.0.lock().unwrap();
                account::get_credentials(&conn, id)?
            };
            account::build_authed_url(&provider, &username, &token, &url)
        }
        None => url.clone(),
    };
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
    // 用账号 token 克隆时禁用凭据助手:认证只走 URL 内嵌的 token,
    // 避免 GCM 把 token 存进系统凭据管理器;后续 pull/push 由用户自己的凭据解决
    if clone_url != url {
        command.arg("-c").arg("credential.helper=");
    }
    command
        .args(["clone", "--", &clone_url, &target_path])
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

    // stderr 由独立任务持续消费,避免管道写满阻塞子进程;
    // 只保留末尾 8KB(进度行很长,且只需末尾的失败原因)
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tauri::async_runtime::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut text = buf.lock().unwrap();
                        text.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        if text.len() > 8192 {
                            let mut cut = text.len() - 4096;
                            while !text.is_char_boundary(cut) {
                                cut += 1;
                            }
                            text.drain(..cut);
                        }
                    }
                }
            }
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
                let detail = stderr_buf.lock().unwrap().trim().to_string();
                break Err(if detail.is_empty() {
                    AppError::coded(ErrorCode::GitCloneFailed, "")
                } else {
                    friendly_git_error(&detail)
                });
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
    // 用账号凭据克隆成功后,把 origin 重置为干净 URL(token 不留在 .git/config)
    if result.is_ok() && clone_url != url {
        let _ = run_git(&target_path, &["remote", "set-url", "origin", &url]);
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
                    .and_then(|remote| remote.url().map(String::from))
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
