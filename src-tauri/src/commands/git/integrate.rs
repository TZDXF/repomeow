use super::*;

/// 将指定分支合并进目标分支(target 缺省为 path 所在工作区的当前分支);
/// squash 时只暂存不自动提交(由用户确认后手动提交)。
/// 目标分支检出在某个 worktree(含主工作区)时在该工作区内合并;未被任何 worktree
/// 检出时只允许快进(预检祖先后 `git branch -f`,不产生工作区改动),分叉或要求
/// squash 时报 git_merge_needs_checkout。
/// 与 pull 一致:产生冲突不算失败,返回冲突文件列表由前端引导解决
#[tauri::command]
pub async fn git_merge(
    app: AppHandle,
    path: String,
    branch: String,
    target: Option<String>,
    squash: bool,
) -> AppResult<GitMergeResult> {
    let event_path = path.clone();
    let result =
        run_blocking(move || merge_blocking(&path, &branch, target.as_deref(), squash)).await?;
    let changed_path = if result.merged_in.is_empty() {
        &event_path
    } else {
        &result.merged_in
    };
    publish_write_status(&app, changed_path, &result.status, "merge", true);
    Ok(result)
}

pub(super) fn merge_blocking(
    path: &str,
    branch: &str,
    target: Option<&str>,
    squash: bool,
) -> AppResult<GitMergeResult> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    // 合并执行位置:目标分支检出在哪个 worktree(含主工作区)就在哪里合并
    let mut merge_path = path.to_string();
    if let Some(t) = target.map(str::trim).filter(|s| !s.is_empty()) {
        if t == branch {
            return Err(AppError::coded(
                ErrorCode::GitCommandFailed,
                "merge: source and target are the same branch",
            ));
        }
        if current_branch(path).as_deref() != Some(t) {
            let wts = list_worktrees_blocking(path)?;
            match wts.iter().find(|w| w.branch.as_deref() == Some(t)) {
                Some(w) => merge_path = w.path.clone(),
                None => {
                    // 目标分支未被任何 worktree 检出:仅允许快进(移动分支指针,
                    // 不动工作区;squash 需要暂存工作区改动,此场景无意义)
                    if squash || !is_ancestor(path, t, branch)? {
                        return Err(AppError::coded(ErrorCode::GitMergeNeedsCheckout, t));
                    }
                    run_git(path, &["branch", "-f", t, branch])?;
                    let st = status(path)?;
                    cache_status(path, &st);
                    return Ok(GitMergeResult {
                        status: st,
                        conflicts: vec![],
                        merged_in: String::new(),
                    });
                }
            }
        }
    }
    let args: Vec<&str> = if squash {
        vec!["merge", "--squash", branch]
    } else {
        vec!["merge", branch]
    };
    let result = git_command(&merge_path).args(&args).output()?;
    let conflicts = unmerged_files(&merge_path);
    if !result.status.success() && conflicts.is_empty() {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(if detail.is_empty() {
            AppError::coded(ErrorCode::GitCommandFailed, "")
        } else {
            friendly_git_error(&detail)
        });
    }
    let st = status(&merge_path)?;
    cache_status(&merge_path, &st);
    Ok(GitMergeResult {
        status: st,
        conflicts,
        merged_in: merge_path,
    })
}

/// 中止进行中的合并(`git merge --abort`),返回最新状态
#[tauri::command]
pub async fn git_merge_abort(app: AppHandle, path: String) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || {
        run_git(&path, &["merge", "--abort"])?;
        let st = status(&path)?;
        cache_status(&path, &st);
        Ok(st)
    })
    .await?;
    publish_write_status(&app, &event_path, &status, "merge_abort", true);
    Ok(status)
}

/// 变基是否处于中断状态:git dir 下存在 rebase-merge / rebase-apply 目录。
/// 用 --absolute-git-dir 兼容 worktree(其 .git 是指向主仓库 gitdir 的文件)
pub(super) fn rebase_in_progress(path: &str) -> bool {
    let Ok(out) = run_git(path, &["rev-parse", "--absolute-git-dir"]) else {
        return false;
    };
    let gitdir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if gitdir.is_empty() {
        return false;
    }
    let gd = Path::new(&gitdir);
    gd.join("rebase-merge").exists() || gd.join("rebase-apply").exists()
}

/// 将当前分支变基到 onto 之上。冲突/中断不算失败:返回冲突文件与 in_progress,
/// 由前端引导用户外部解决后 --continue,或调用 git_rebase_abort 中止
#[tauri::command]
pub async fn git_rebase(app: AppHandle, path: String, onto: String) -> AppResult<GitRebaseResult> {
    let event_path = path.clone();
    let result = run_blocking(move || rebase_blocking(&path, &onto)).await?;
    publish_write_status(&app, &event_path, &result.status, "rebase", true);
    Ok(result)
}

pub(super) fn rebase_blocking(path: &str, onto: &str) -> AppResult<GitRebaseResult> {
    let onto = onto.trim();
    if onto.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    let result = git_command(path).args(["rebase", onto]).output()?;
    let conflicts = unmerged_files(path);
    let in_progress = rebase_in_progress(path);
    if !result.status.success() && conflicts.is_empty() && !in_progress {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(if detail.is_empty() {
            AppError::coded(ErrorCode::GitCommandFailed, "")
        } else {
            friendly_git_error(&detail)
        });
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(GitRebaseResult {
        status: st,
        conflicts,
        in_progress,
    })
}

/// 中止进行中的变基(`git rebase --abort`),返回最新状态
#[tauri::command]
pub async fn git_rebase_abort(app: AppHandle, path: String) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || {
        run_git(&path, &["rebase", "--abort"])?;
        let st = status(&path)?;
        cache_status(&path, &st);
        Ok(st)
    })
    .await?;
    publish_write_status(&app, &event_path, &status, "rebase_abort", true);
    Ok(status)
}
