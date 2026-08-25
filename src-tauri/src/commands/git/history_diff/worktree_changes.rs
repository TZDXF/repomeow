use super::{super::*, apply_display_opts, truncate_chars, COMMIT_DIFF_MAX_CHARS};

/// 构建工作区相对 HEAD 的 diff(覆盖已暂存 + 已跟踪未暂存修改 + 未跟踪文件,与 git_commit 语义一致);
/// 仓库尚无提交(无 HEAD)时回退到暂存区 diff(相对空树);
/// include_untracked 使未跟踪文件以 Added delta 出现,补丁内容直接读工作区
fn worktree_diff<'r>(
    repo: &'r Repository,
    configure: impl FnOnce(&mut DiffOptions),
) -> AppResult<git2::Diff<'r>> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    // 未跟踪文件以 Untracked delta 出现(不是 Added);
    // show_untracked_content 使补丁能读到未跟踪文件内容(自动开启 include_untracked)
    opts.include_typechange(true)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    configure(&mut opts);
    let mut diff = match &head_tree {
        Some(tree) => repo.diff_tree_to_workdir_with_index(Some(tree), Some(&mut opts)),
        // 无 HEAD(未出生分支):空树对工作区+索引。不能用 diff_tree_to_index:
        // 它不涉及工作目录,include_untracked 无效,新仓库里的未跟踪文件会全部漏掉
        None => repo.diff_tree_to_workdir_with_index(None, Some(&mut opts)),
    }
    .map_err(git_err)?;
    diff.find_similar(Some(&mut DiffFindOptions::new().renames(true)))
        .map_err(git_err)?;
    Ok(diff)
}

/// 读取工作区待提交的变更文件清单(状态 + 增删行数,提交对话框变更预览用)。
/// 未跟踪文件以 status A + untracked 标记返回;嵌套 git 仓库始终排除(与 git_commit 一致)
pub(crate) async fn git_worktree_files(path: String) -> AppResult<Vec<GitWorktreeFile>> {
    run_blocking(move || worktree_files_blocking(&path)).await
}

pub(crate) fn worktree_files_blocking(path: &str) -> AppResult<Vec<GitWorktreeFile>> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let diff = worktree_diff(&repo, |_| {})?;
    let index = repo.index().map_err(git_err)?;
    let workdir = repo
        .workdir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let mut nested_cache: HashSet<String> = HashSet::new();
    let mut files = Vec::new();
    for (idx, delta) in diff.deltas().enumerate() {
        let status = match delta.status() {
            // 未跟踪文件是 Untracked 而非 Added,统一按新增展示(untracked 标记另行区分)
            Delta::Added | Delta::Untracked => 'A',
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
            .map(|p| crate::path_util::to_forward_slash_str(&p.to_string_lossy()))
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        // 嵌套 git 仓库是独立项目,不算本仓库待提交内容
        if is_nested_repo_cached(&workdir, &path, &mut nested_cache) {
            continue;
        }
        let old_path = if matches!(status, 'R' | 'C') {
            delta
                .old_file()
                .path()
                .map(|p| crate::path_util::to_forward_slash_str(&p.to_string_lossy()))
        } else {
            None
        };
        // 新增文件不在索引中即未跟踪(已暂存的新文件在索引里)
        let untracked = status == 'A' && index.get_path(Path::new(&path), 0).is_none();
        // 增删行数;二进制行数记 None(delta 的 binary 标志要在 Patch 加载内容后才置位)
        let patch = Patch::from_diff(&diff, idx).ok().flatten();
        let is_binary = diff
            .get_delta(idx)
            .map(|d| d.flags().is_binary())
            .unwrap_or(false);
        let (additions, deletions) = if is_binary {
            (None, None)
        } else {
            match patch {
                None => (Some(0), Some(0)),
                Some(p) => p
                    .line_stats()
                    .ok()
                    .map(|(_, a, d)| (Some(a as u32), Some(d as u32)))
                    .unwrap_or((None, None)),
            }
        };
        files.push(GitWorktreeFile {
            path,
            old_path,
            status: status.to_string(),
            additions,
            deletions,
            untracked,
        });
    }
    Ok(files)
}

/// 读取工作区单个待提交文件的 diff(相对 HEAD;未跟踪文件为全新增补丁)。
/// 重命名时新旧路径都作为 pathspec 传入。超长按字符截断。
/// context_lines 与 ignore_ws 语义同 git_commit_file_diff(前端 DiffViewer 共用展示语义)
pub(crate) async fn git_worktree_file_diff(
    path: String,
    file_path: String,
    old_path: Option<String>,
    ignore_ws: Option<String>,
) -> AppResult<GitCommitFileDiff> {
    run_blocking(move || {
        worktree_file_diff_blocking(&path, &file_path, old_path.as_deref(), ignore_ws.as_deref())
    })
    .await
}

pub(crate) fn worktree_file_diff_blocking(
    path: &str,
    file_path: &str,
    old_path: Option<&str>,
    ignore_ws: Option<&str>,
) -> AppResult<GitCommitFileDiff> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let diff = worktree_diff(&repo, |opts| {
        opts.pathspec(file_path);
        apply_display_opts(opts, ignore_ws);
        if let Some(old) = old_path {
            opts.pathspec(old);
        }
    })?;
    let mut text = String::new();
    for (idx, _) in diff.deltas().enumerate() {
        if let Ok(Some(mut patch)) = Patch::from_diff(&diff, idx) {
            if let Ok(buf) = patch.to_buf() {
                text.push_str(&String::from_utf8_lossy(&buf));
            }
        }
    }
    let (diff, truncated) = truncate_chars(&text, COMMIT_DIFF_MAX_CHARS);
    Ok(GitCommitFileDiff { diff, truncated })
}
