use super::*;

/// 列出所有 remote 及其地址(非仓库或无 remote 返回空列表)
#[tauri::command]
pub async fn list_git_remotes(path: String) -> AppResult<Vec<GitRemote>> {
    run_blocking(move || list_remotes_blocking(&path)).await
}

pub(super) fn list_remotes_blocking(path: &str) -> AppResult<Vec<GitRemote>> {
    // 非仓库或无 remote 返回空列表
    let Some(repo) = open_repo(path)? else {
        return Ok(vec![]);
    };
    let names = repo.remotes().map_err(git_err)?;
    let mut out: Vec<GitRemote> = Vec::new();
    for name in names.iter().filter_map(Result::ok).flatten() {
        // 无 URL 的 remote(纯 pushurl 等)跳过,与 `git remote -v` 一致取 fetch 地址
        let url = repo
            .find_remote(name)
            .ok()
            .and_then(|r| r.url().ok().map(String::from));
        if let Some(url) = url.filter(|u| !u.is_empty()) {
            out.push(GitRemote {
                name: name.to_string(),
                url,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 带超时的后台 fetch:
/// - http(s) 协议由 git 连接/低速超时配置兜底(慢速连接 30s 无进展即中止)
/// - ssh 等其他协议由外层 timeout 兜底,超时 kill 进程树
/// 无 remote 的仓库直接视为成功(无需退避)
pub(super) async fn fetch_with_timeout(path: &str) -> bool {
    if !repo_has_remote(path) {
        return true;
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args([
            "-c",
            "http.connectTimeout=10",
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=30",
            "fetch",
            "--quiet",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // tokio::process::Command 在 Windows 上原生提供 creation_flags,无需 import
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    // 登记 PID 供应用退出钩子按 PID 清理(句柄在当前 task 内部,退出钩子够不到)
    let _tracked = TrackedPid::new(child.id());
    match tokio::time::timeout(FETCH_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            // 超时:强制结束进程树(fetch 会派生 remote helper 孙进程)
            kill_process_tree(child);
            false
        }
    }
}

/// 强制结束 git 进程树(Windows 用 taskkill /T /F 覆盖孙进程)
pub(super) fn kill_process_tree(mut child: tokio::process::Child) {
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
    let _ = child.wait();
}

/// 前台 fetch(带超时与进程树清理):`args` 追加在 `fetch` 之后。
/// 与后台 `fetch_with_timeout` 的区别:捕获 stderr 走 friendly_git_error,让界面能
/// 给出认证失败/远端分支已删除这类可操作提示;后台版失败静默退避,不打扰用户。
/// 无 remote 的仓库直接视为成功(没有可刷新的远端引用)。
pub(super) async fn fetch_args(path: &str, args: &[&str]) -> AppResult<()> {
    if !repo_has_remote(path) {
        return Ok(());
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args([
            "-c",
            "http.connectTimeout=10",
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=30",
            "fetch",
            "--quiet",
        ])
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // tokio::process::Command 在 Windows 上原生提供 creation_flags,无需 import
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn()?;
    // 登记 PID 供应用退出钩子按 PID 清理(句柄在当前 task 内部,退出钩子够不到)
    let _tracked = TrackedPid::new(child.id());
    // stderr 由独立 task 持续排空:fetch 会写进度与错误,管道写满会阻塞子进程
    let drain = child.stderr.take().map(|mut pipe| {
        use tokio::io::AsyncReadExt;
        tokio::spawn(async move {
            let mut text = String::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                text.push_str(&String::from_utf8_lossy(&chunk[..n]));
            }
            text
        })
    });
    let status = match tokio::time::timeout(FETCH_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            // 超时:强制结束进程树(fetch 会派生 remote helper 孙进程)
            kill_process_tree(child);
            return Err(AppError::coded(
                ErrorCode::GitCommandFailed,
                format!("fetch timeout {}s", FETCH_TIMEOUT.as_secs()),
            ));
        }
    };
    let stderr = match drain {
        Some(handle) => handle.await.unwrap_or_default(),
        None => String::new(),
    };
    if status.success() {
        return Ok(());
    }
    Err(friendly_git_error(&stderr))
}

/// fetch 成功后的最新状态:回填缓存并标记 last_fetch_at,与后台检查保持一致
pub(super) fn refresh_after_fetch(path: &str) -> AppResult<GitStatus> {
    let mut st = status(path)?;
    st.last_fetch_at = Some(crate::time_util::now_ts());
    cache_status(path, &st);
    Ok(st)
}

/// 刷新单个远程分支的跟踪引用(`+refs/heads/<short>:refs/remotes/<remote>/<short>`)。
/// 只更新该分支的远端引用,不切工作区、不合并:界面上远程分支的「更新」即此语义。
/// 远端分支已被删除时定向 fetch 会报 couldn't find remote ref;此时顺手对同一远端
/// prune 一次(顺手清掉该远端其它已删除分支的陈旧引用),再原样返回错误。
pub(super) async fn fetch_remote_branch(path: &str, branch: &str) -> AppResult<GitStatus> {
    let (remote, short) = split_remote_branch(branch.trim())
        .ok_or_else(|| AppError::coded(ErrorCode::GitBranchNameRequired, ""))?;
    let refspec = format!("+refs/heads/{short}:refs/remotes/{remote}/{short}");
    if let Err(e) = fetch_args(path, &[&remote, &refspec]).await {
        if matches!(
            e,
            AppError::Coded {
                code: ErrorCode::GitRemoteBranchGone,
                ..
            }
        ) {
            let _ = fetch_args(path, &["--prune", "--", &remote]).await;
        }
        return Err(e);
    }
    refresh_after_fetch(path)
}

/// 刷新全部远程分支(`fetch --prune --all`),同时清理远端已删除分支的陈旧引用
pub(super) async fn fetch_all_remotes(path: &str) -> AppResult<GitStatus> {
    fetch_args(path, &["--prune", "--all"]).await?;
    refresh_after_fetch(path)
}

/// 更新单个远程分支的跟踪引用(界面「更新」):不切换工作区、不合并
#[tauri::command]
pub async fn git_remote_branch_fetch(
    app: AppHandle,
    path: String,
    branch: String,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = fetch_remote_branch(&path, &branch).await?;
    publish_write_status(&app, &event_path, &status, "remote_branch_fetch", false);
    Ok(status)
}

/// 更新全部远程分支(远程分支区标题行的「全部更新」)
#[tauri::command]
pub async fn git_remote_branches_fetch(app: AppHandle, path: String) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = fetch_all_remotes(&path).await?;
    publish_write_status(&app, &event_path, &status, "remote_branches_fetch", false);
    Ok(status)
}

/// `git merge --ff-only @{u}`:仅快进合并,不可能产生合并提交。
/// 返回是否快进成功;失败一律视为「取消」(不留状态、不提醒)
pub(super) fn ff_pull_blocking(path: &str) -> bool {
    git_command(path)
        .args(["merge", "--ff-only", "@{u}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 当前处于合并冲突状态的文件(相对仓库根的路径)
pub(crate) fn unmerged_files(path: &str) -> Vec<String> {
    let Ok(out) = git_command(path)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub(super) fn local_branch_names(path: &str) -> AppResult<Vec<String>> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    local_branch_names_of(&repo)
}

/// 本地分支短名列表(剥 refs/heads/ 前缀,与 git log %D 装饰命名一致;
/// 不用 short 消歧规则,避免分支与 remote 同名时输出 "heads/zc" 导致图谱定位失败)
pub(super) fn local_branch_names_of(repo: &Repository) -> AppResult<Vec<String>> {
    let iter = repo.branches(Some(BranchType::Local)).map_err(git_err)?;
    let mut names = Vec::new();
    for item in iter {
        let (branch, _) = item.map_err(git_err)?;
        if let Some(name) = branch.name().ok().flatten() {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

#[tauri::command]
pub async fn list_git_branches(path: String) -> AppResult<GitBranches> {
    run_blocking(move || list_branches_blocking(&path)).await
}

pub(super) fn list_branches_blocking(path: &str) -> AppResult<GitBranches> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let local = local_branch_names_of(&repo)?;

    // 远程分支:剥 refs/remotes/ 前缀,过滤 origin/HEAD 这类符号引用
    let mut remote = Vec::new();
    let remote_iter = repo.branches(Some(BranchType::Remote)).map_err(git_err)?;
    for item in remote_iter {
        let (branch, _) = item.map_err(git_err)?;
        if branch.get().kind() == Some(git2::ReferenceType::Symbolic) {
            continue;
        }
        if let Some(name) = branch.name().ok().flatten() {
            remote.push(name.to_string());
        }
    }
    remote.sort();

    // 本地分支的 upstream 跟踪:未配置不收录;上游引用已删除([gone])记 upstream=None
    let mut tracking = Vec::new();
    for name in &local {
        let branch_ref = format!("refs/heads/{name}");
        let Ok(upstream_buf) = repo.branch_upstream_name(&branch_ref) else {
            continue;
        };
        let upstream_full = String::from_utf8_lossy(&upstream_buf).to_string();
        let upstream_short = upstream_full
            .strip_prefix("refs/remotes/")
            .or_else(|| upstream_full.strip_prefix("refs/heads/"))
            .unwrap_or(&upstream_full)
            .to_string();
        let upstream_oid = repo
            .find_reference(&upstream_full)
            .ok()
            .and_then(|r| r.target());
        match upstream_oid {
            // 上游引用解析失败(已删除):与原 %(upstream:track)=[gone] 一致
            None => tracking.push(GitBranchTrack {
                name: name.clone(),
                upstream: None,
                ahead: 0,
                behind: 0,
            }),
            Some(up_oid) => {
                let (ahead, behind) = repo
                    .find_reference(&branch_ref)
                    .ok()
                    .and_then(|r| r.target())
                    .and_then(|local| repo.graph_ahead_behind(local, up_oid).ok())
                    .unwrap_or((0, 0));
                tracking.push(GitBranchTrack {
                    name: name.clone(),
                    upstream: Some(upstream_short),
                    ahead: ahead as u32,
                    behind: behind as u32,
                });
            }
        }
    }
    Ok(GitBranches {
        local,
        remote,
        tracking,
    })
}
