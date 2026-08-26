use super::*;

/// 构造 git 命令:禁用终端凭据交互(GUI 应用无人应答会挂起,凭据管理器
/// helper 弹窗不受影响),Windows 下隐藏控制台黑窗
pub(super) fn git_command_raw() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

pub(crate) fn git_command(path: &str) -> Command {
    let mut cmd = git_command_raw();
    cmd.arg("-C").arg(path);
    cmd
}

/// 执行 git 命令,非零退出时取 stderr(兜底 stdout)转为友好错误
pub(crate) fn run_git(path: &str, args: &[&str]) -> AppResult<Output> {
    let output = git_command(path).args(args).output()?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(if detail.is_empty() {
        AppError::coded(
            ErrorCode::GitCommandFailed,
            format!("args={} status={}", args.join(" "), output.status),
        )
    } else {
        friendly_git_error(&detail)
    })
}

/// 将 git 原始 stderr 转为简洁友好的错误:
/// 1. 过滤环境噪音行(如 OpenSSH 后量子密钥交换警告)
/// 2. 常见错误模式映射为带错误码的 Coded 错误(前端按 code 走 i18n,
///    此处 message 仅保留技术上下文);未识别时返回清理后的原文(External→Coded)
///
/// 注意:`push_blocking` 依赖原文匹配 "no upstream branch",映射规则不得覆盖该短语
pub(super) fn friendly_git_error(raw: &str) -> AppError {
    use crate::error::ErrorCode;

    // 噪音行:SSH/网络层打印的警告,与 git 操作结果无关
    const NOISE: &[&str] = &[
        "post-quantum",
        "store now, decrypt later",
        "openssh.com/pq.html",
        "The server may need to be upgraded",
    ];
    let cleaned: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("** "))
        .filter(|l| !NOISE.iter().any(|n| l.contains(n)))
        .collect();
    let text = cleaned.join("\n");
    if text.is_empty() {
        return AppError::coded(ErrorCode::GitNoiseFallback, "");
    }
    let coded = |code: ErrorCode, message: &str| AppError::Coded {
        code,
        message: message.into(),
    };

    // 本地修改/未跟踪文件会被合并或切换分支覆盖
    if text.contains("Your local changes to the following files would be overwritten by") {
        return coded(ErrorCode::GitLocalChangesConflict, "");
    }
    if text.contains("The following untracked working tree files would be overwritten by") {
        return coded(ErrorCode::GitUntrackedConflict, "");
    }

    // 认证与权限
    if text.contains("Permission denied (publickey") {
        return coded(ErrorCode::GitSshAuthFailed, "");
    }
    if text.contains("Host key verification failed") {
        return coded(ErrorCode::GitHostKeyFailed, "");
    }
    if text.contains("Authentication failed") || text.contains("Invalid username or password") {
        return coded(ErrorCode::GitAuthFailed, "");
    }
    // 远端仓库不存在或当前账号无权限(私有库认证失败时服务端同样回 not found 而非 403,
    // 故置于认证类之后无碍:git 若明确报 "Authentication failed" 已在上面优先命中):
    //   - GitHub:"Repository not found." / "fatal: repository '<url>' not found"
    //   - GitLab:"The project you were looking for could not be found."
    //   - 通用:  "fatal: repository '<url>' not found"(URL 夹在 repository 与 not found 之间,
    //            不能用 "repository not found" 连续子串匹配,需拆成两个 contains)
    if text.contains("Repository not found")
        || text.contains("could not be found")
        || (text.contains("repository") && text.contains("not found"))
    {
        return coded(ErrorCode::GitRepoNotFound, "");
    }

    // 网络
    if text.contains("Could not resolve host")
        || text.contains("Temporary failure in name resolution")
    {
        return coded(ErrorCode::GitNetworkDns, "");
    }
    if text.contains("Connection timed out")
        || text.contains("Connection refused")
        || text.contains("Connection reset")
        || text.contains("Failed to connect to")
    {
        return coded(ErrorCode::GitNetworkConnect, "");
    }

    // 推送/拉取策略
    if text.contains("failed to push some refs") {
        if text.contains("non-fast-forward")
            || text.contains("fetch first")
            || text.contains("Updates were rejected")
        {
            return coded(ErrorCode::GitPushRejected, "");
        }
        return AppError::coded(ErrorCode::GitPushFailed, text);
    }
    if text.contains("You have divergent branches")
        || text.contains("Need to specify how to reconcile divergent branches")
    {
        return coded(ErrorCode::GitDiverged, "");
    }
    // 上游远程分支已被删除:pull 当前分支时报 "no such ref was fetched",
    // fetch/pull 指定分支时报 "couldn't find remote ref"(不同 git 版本大小写不一)
    if text.contains("no such ref was fetched")
        || text
            .to_ascii_lowercase()
            .contains("couldn't find remote ref")
    {
        return coded(ErrorCode::GitRemoteBranchGone, "");
    }
    if text.contains("There is no tracking information") {
        return coded(ErrorCode::GitNoTracking, "");
    }
    if text.contains("not a git repository") {
        return coded(ErrorCode::NotGitRepository, "");
    }

    // worktree / 分支占用
    // fetch refspec 更新被 worktree 检出的分支(拉取非当前分支时):
    // "fatal: refusing to fetch into branch 'refs/heads/x' checked out at '<path>'"
    // message 提取 worktree 路径透出,指引用户到该 worktree 拉取
    if text.contains("refusing to fetch into branch") {
        let worktree_path = text
            .split("checked out at '")
            .nth(1)
            .and_then(|rest| rest.split('\'').next())
            .unwrap_or("");
        return coded(ErrorCode::GitFetchIntoCheckedOut, worktree_path);
    }
    if text.contains("is already checked out at") {
        return coded(ErrorCode::GitBranchCheckedOut, "");
    }
    if text.contains("contains modified or untracked files") {
        return coded(ErrorCode::GitWorktreeDirty, "");
    }
    if text.contains("branch named") && text.contains("already exists") {
        return coded(ErrorCode::GitBranchExists, "");
    }
    // 删除分支:未完全合并(git 建议 -D 强删),前端据此引导强制删除
    if text.contains("is not fully merged") {
        return coded(ErrorCode::GitBranchNotMerged, "");
    }

    // 未识别:整段清理后原文作为 message 携带
    AppError::coded(ErrorCode::GitCommandFailed, text)
}

/// 阻塞任务放入 tokio 线程池执行。
/// 同步 #[tauri::command] 在主线程跑,git 子进程(尤其 push/pull 网络操作)会卡死 UI
pub(super) async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::coded(ErrorCode::GitTaskFailed, e.to_string()))?
}
