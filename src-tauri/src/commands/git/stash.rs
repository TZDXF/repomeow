use super::*;

/// 使用 libgit2 读取 stash 列表，不创建 git 子进程。
#[tauri::command]
pub async fn list_git_stashes(path: String) -> AppResult<Vec<GitStash>> {
    run_blocking(move || list_stashes_blocking(&path)).await
}

pub(super) fn list_stashes_blocking(path: &str) -> AppResult<Vec<GitStash>> {
    let Some(mut repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };

    // stash_foreach 需要 &mut Repository；先收集轻量字段，再读取提交元数据，
    // 避免在回调中同时借用 repo。
    let mut raw = Vec::new();
    repo.stash_foreach(|index, message, oid| {
        raw.push((index, message.to_string(), *oid));
        true
    })
    .map_err(git_err)?;

    let mut stashes = Vec::with_capacity(raw.len());
    for (index, message, oid) in raw {
        let commit = repo.find_commit(oid).map_err(git_err)?;
        stashes.push(GitStash {
            index,
            oid: oid.to_string(),
            message,
            author: commit.author().name().unwrap_or_default().to_string(),
            created_at: commit.time().seconds(),
        });
    }
    stashes.sort_by_key(|stash| stash.index);
    Ok(stashes)
}

/// 读取指定 stash 的文件清单。stash 是多父提交：第一个父提交到 stash 树表示
/// 已跟踪文件变化，第三个父提交（存在时）单独保存 --include-untracked 的文件。
#[tauri::command]
pub async fn git_stash_files(path: String, oid: String) -> AppResult<Vec<GitCommitFile>> {
    run_blocking(move || stash_files_blocking(&path, &oid)).await
}

pub(super) fn stash_files_blocking(path: &str, oid: &str) -> AppResult<Vec<GitCommitFile>> {
    let Some(mut repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let oid = verify_stash_oid(&mut repo, oid)?;
    let commit = repo.find_commit(oid).map_err(git_err)?;
    let diffs = stash_diffs(&repo, &commit, None, None, None)?;

    let mut files = Vec::new();
    for diff in diffs {
        for (idx, delta) in diff.deltas().enumerate() {
            let status = match delta.status() {
                Delta::Added => 'A',
                Delta::Copied => 'C',
                Delta::Deleted => 'D',
                Delta::Modified => 'M',
                Delta::Renamed => 'R',
                Delta::Typechange => 'T',
                _ => continue,
            };
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|path| crate::path_util::to_forward_slash_str(&path.to_string_lossy()))
                .unwrap_or_default();
            if path.is_empty() {
                continue;
            }
            let old_path = if matches!(status, 'R' | 'C') {
                delta
                    .old_file()
                    .path()
                    .map(|path| crate::path_util::to_forward_slash_str(&path.to_string_lossy()))
            } else {
                None
            };
            let patch = Patch::from_diff(&diff, idx).ok().flatten();
            let is_binary = diff
                .get_delta(idx)
                .map(|item| item.flags().is_binary())
                .unwrap_or(false);
            let (additions, deletions) = if is_binary {
                (None, None)
            } else {
                match patch {
                    None => (Some(0), Some(0)),
                    Some(patch) => patch
                        .line_stats()
                        .ok()
                        .map(|(_, additions, deletions)| {
                            (Some(additions as u32), Some(deletions as u32))
                        })
                        .unwrap_or((None, None)),
                }
            };
            files.push(GitCommitFile {
                path,
                old_path,
                status: status.to_string(),
                additions,
                deletions,
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// 读取 stash 中单个文件的完整上下文 diff，供前端 DiffViewer 展示。
#[tauri::command]
pub async fn git_stash_file_diff(
    path: String,
    oid: String,
    file_path: String,
    old_path: Option<String>,
    ignore_ws: Option<String>,
) -> AppResult<GitCommitFileDiff> {
    run_blocking(move || {
        stash_file_diff_blocking(
            &path,
            &oid,
            &file_path,
            old_path.as_deref(),
            ignore_ws.as_deref(),
        )
    })
    .await
}

pub(super) fn stash_file_diff_blocking(
    path: &str,
    oid: &str,
    file_path: &str,
    old_path: Option<&str>,
    ignore_ws: Option<&str>,
) -> AppResult<GitCommitFileDiff> {
    let Some(mut repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let oid = verify_stash_oid(&mut repo, oid)?;
    let commit = repo.find_commit(oid).map_err(git_err)?;
    let diffs = stash_diffs(&repo, &commit, Some(file_path), old_path, ignore_ws)?;
    let mut text = String::new();
    for diff in diffs {
        for (idx, _) in diff.deltas().enumerate() {
            if let Ok(Some(mut patch)) = Patch::from_diff(&diff, idx) {
                if let Ok(buffer) = patch.to_buf() {
                    text.push_str(&String::from_utf8_lossy(&buffer));
                }
            }
        }
    }
    let (diff, truncated) =
        super::history_diff::truncate_chars(&text, super::history_diff::COMMIT_DIFF_MAX_CHARS);
    Ok(GitCommitFileDiff { diff, truncated })
}

fn verify_stash_oid(repo: &mut Repository, oid: &str) -> AppResult<git2::Oid> {
    let expected = git2::Oid::from_str(oid).map_err(git_err)?;
    let mut found = false;
    repo.stash_foreach(|_, _, current| {
        if *current == expected {
            found = true;
            false
        } else {
            true
        }
    })
    .map_err(git_err)?;
    if !found {
        return Err(AppError::coded(ErrorCode::GitStashChanged, ""));
    }
    Ok(expected)
}

fn stash_diffs<'repo>(
    repo: &'repo Repository,
    commit: &git2::Commit<'repo>,
    file_path: Option<&str>,
    old_path: Option<&str>,
    ignore_ws: Option<&str>,
) -> AppResult<Vec<git2::Diff<'repo>>> {
    let stash_tree = commit.tree().map_err(git_err)?;
    let base_tree = commit
        .parent(0)
        .and_then(|parent| parent.tree())
        .map_err(git_err)?;
    let mut diffs = Vec::with_capacity(if commit.parent_count() >= 3 { 2 } else { 1 });

    let mut tracked_opts = stash_diff_options(file_path, old_path, ignore_ws);
    let mut tracked = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&stash_tree), Some(&mut tracked_opts))
        .map_err(git_err)?;
    tracked
        .find_similar(Some(&mut DiffFindOptions::new().renames(true)))
        .map_err(git_err)?;
    diffs.push(tracked);

    if commit.parent_count() >= 3 {
        let untracked_tree = commit
            .parent(2)
            .and_then(|parent| parent.tree())
            .map_err(git_err)?;
        let mut untracked_opts = stash_diff_options(file_path, old_path, ignore_ws);
        let untracked = repo
            .diff_tree_to_tree(None, Some(&untracked_tree), Some(&mut untracked_opts))
            .map_err(git_err)?;
        diffs.push(untracked);
    }
    Ok(diffs)
}

fn stash_diff_options(
    file_path: Option<&str>,
    old_path: Option<&str>,
    ignore_ws: Option<&str>,
) -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.include_typechange(true);
    if let Some(path) = file_path {
        opts.pathspec(path);
        super::history_diff::apply_display_opts(&mut opts, ignore_ws);
    }
    if let Some(path) = old_path {
        opts.pathspec(path);
    }
    opts
}

/// 创建 stash。写操作使用系统 Git CLI，以保持与用户 Git 环境一致。
#[tauri::command]
pub async fn git_stash_push(
    app: AppHandle,
    path: String,
    message: String,
    include_untracked: bool,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let result =
        run_blocking(move || stash_push_blocking(&path, &message, include_untracked)).await;
    finish_stash_write(&app, &event_path, "stash_push", result).await
}

pub(super) fn stash_push_blocking(
    path: &str,
    message: &str,
    include_untracked: bool,
) -> AppResult<GitStatus> {
    let current_status = status(path)?;
    let has_tracked_changes = current_status.staged > 0 || current_status.modified > 0;
    if !has_tracked_changes && !(include_untracked && current_status.untracked > 0) {
        return Err(AppError::coded(ErrorCode::GitStashNothingToSave, ""));
    }

    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("--include-untracked");
    }
    let message = message.trim();
    if !message.is_empty() {
        args.extend(["-m", message]);
    }
    run_git(path, &args)?;

    let current_status = status(path)?;
    cache_status(path, &current_status);
    Ok(current_status)
}

/// 弹出指定 stash。oid 是列表读取时的快照保护，防止列表变化后 index 指向另一条记录。
#[tauri::command]
pub async fn git_stash_pop(
    app: AppHandle,
    path: String,
    index: usize,
    oid: String,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let result = run_blocking(move || stash_write_blocking(&path, index, &oid, "pop")).await;
    finish_stash_write(&app, &event_path, "stash_pop", result).await
}

/// 清理指定 stash，不应用其中内容。
#[tauri::command]
pub async fn git_stash_drop(
    app: AppHandle,
    path: String,
    index: usize,
    oid: String,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let result = run_blocking(move || stash_write_blocking(&path, index, &oid, "drop")).await;
    finish_stash_write(&app, &event_path, "stash_drop", result).await
}

pub(super) fn stash_write_blocking(
    path: &str,
    index: usize,
    expected_oid: &str,
    action: &str,
) -> AppResult<GitStatus> {
    let current = list_stashes_blocking(path)?;
    let unchanged = current
        .iter()
        .any(|stash| stash.index == index && stash.oid == expected_oid);
    if !unchanged {
        return Err(AppError::coded(ErrorCode::GitStashChanged, ""));
    }

    let stash_ref = format!("stash@{{{index}}}");
    run_git(path, &["stash", action, &stash_ref])?;
    let current_status = status(path)?;
    cache_status(path, &current_status);
    Ok(current_status)
}

/// pop 冲突时 CLI 会以失败退出，但工作区可能已经改变；失败路径也刷新并发布状态。
async fn finish_stash_write(
    app: &AppHandle,
    path: &str,
    source: &str,
    result: AppResult<GitStatus>,
) -> AppResult<GitStatus> {
    match result {
        Ok(current_status) => {
            publish_write_status(app, path, &current_status, source, false);
            Ok(current_status)
        }
        Err(error) => {
            let refresh_path = path.to_string();
            if let Ok(current_status) = run_blocking(move || {
                let current_status = status(&refresh_path)?;
                cache_status(&refresh_path, &current_status);
                Ok(current_status)
            })
            .await
            {
                publish_write_status(app, path, &current_status, source, false);
            }
            Err(error)
        }
    }
}
