use super::{super::*, apply_display_opts, truncate_chars, COMMIT_DIFF_MAX_CHARS};

/// 读取某次提交的树间 diff(详情面板文件清单与单文件 diff 共用)。
/// 根提交相对空树(等价 diff-tree --root);-M 重命名识别;
/// 合并提交(多父)无单 diff 语义,返回 None(与原 diff-tree 默认无输出一致)
fn commit_diff<'r>(
    repo: &'r Repository,
    hash: &str,
    configure: impl FnOnce(&mut DiffOptions),
) -> AppResult<Option<git2::Diff<'r>>> {
    let commit = repo
        .revparse_single(hash)
        .and_then(|o| o.peel_to_commit())
        .map_err(git_err)?;
    if commit.parent_count() > 1 {
        return Ok(None);
    }
    let new_tree = commit.tree().map_err(git_err)?;
    let old_tree = if commit.parent_count() == 1 {
        Some(commit.parent(0).and_then(|p| p.tree()).map_err(git_err)?)
    } else {
        None
    };
    let mut opts = DiffOptions::new();
    opts.include_typechange(true);
    configure(&mut opts);
    let mut diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))
        .map_err(git_err)?;
    diff.find_similar(Some(&mut DiffFindOptions::new().renames(true)))
        .map_err(git_err)?;
    Ok(Some(diff))
}

/// 读取某次提交触及的文件清单(状态 + 增删行数,提交详情面板文件列表用)。
/// 合并提交(多父)返回空数组由前端提示
pub(crate) async fn git_commit_files(path: String, hash: String) -> AppResult<Vec<GitCommitFile>> {
    run_blocking(move || commit_files_blocking(&path, &hash)).await
}

pub(crate) fn commit_files_blocking(path: &str, hash: &str) -> AppResult<Vec<GitCommitFile>> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let Some(diff) = commit_diff(&repo, hash, |_| {})? else {
        return Ok(Vec::new());
    };
    let mut files = Vec::new();
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
            .map(|p| crate::path_util::to_forward_slash_str(&p.to_string_lossy()))
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        // 重命名 / 复制:记录旧路径
        let old_path = if matches!(status, 'R' | 'C') {
            delta
                .old_file()
                .path()
                .map(|p| crate::path_util::to_forward_slash_str(&p.to_string_lossy()))
        } else {
            None
        };
        // 增删行数;二进制行数记 None(与 numstat 的 "-" 一致)。
        // 注意:delta 的 binary 标志要在 Patch::from_diff 加载内容后才置位
        let patch = Patch::from_diff(&diff, idx).ok().flatten();
        let is_binary = diff
            .get_delta(idx)
            .map(|d| d.flags().is_binary())
            .unwrap_or(false);
        let (additions, deletions) = if is_binary {
            (None, None)
        } else {
            match patch {
                // Patch 为 None(纯模式变更等空补丁):numstat 记 0 0
                None => (Some(0), Some(0)),
                Some(p) => p
                    .line_stats()
                    .ok()
                    .map(|(_, a, d)| (Some(a as u32), Some(d as u32)))
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
    Ok(files)
}

/// 读取某次提交中单个文件的 diff(提交详情面板用)。
/// 重命名时新旧路径都作为 pathspec 传入。超长按字符截断(二进制 diff 天然很短)
/// context_lines 拉满:前端并排/逐行视图均展示完整文件内容(未更改区间由前端折叠)
/// ignore_ws:前端"忽略空白差异"模式,None/"none" 不忽略
pub(crate) async fn git_commit_file_diff(
    path: String,
    hash: String,
    file_path: String,
    old_path: Option<String>,
    ignore_ws: Option<String>,
) -> AppResult<GitCommitFileDiff> {
    run_blocking(move || {
        commit_file_diff_blocking(
            &path,
            &hash,
            &file_path,
            old_path.as_deref(),
            ignore_ws.as_deref(),
        )
    })
    .await
}

pub(crate) fn commit_file_diff_blocking(
    path: &str,
    hash: &str,
    file_path: &str,
    old_path: Option<&str>,
    ignore_ws: Option<&str>,
) -> AppResult<GitCommitFileDiff> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let Some(diff) = commit_diff(&repo, hash, |opts| {
        opts.pathspec(file_path);
        apply_display_opts(opts, ignore_ws);
        if let Some(old) = old_path {
            opts.pathspec(old);
        }
    })?
    else {
        // 合并提交:前端不会对合并提交调本接口,返回空
        return Ok(GitCommitFileDiff {
            diff: String::new(),
            truncated: false,
        });
    };
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

/// 单文件 blob 的预览上限(超出报错,避免异常大的二进制经 IPC 传输撑爆前端)
const COMMIT_BLOB_MAX_BYTES: usize = 32 * 1024 * 1024;

/// 读取某次提交(或其第一个父提交)中一个文件的 blob 内容,base64 编码返回
/// (提交详情面板拼 data: URL 预览图片)。文件在该版本不存在(如新增文件的旧版本、
/// 根提交请求父版本)返回 None;子模块等非 blob 条目同样返回 None。
/// parent = true 时读父提交版本(旧版),否则读该提交本身(新版);重命名的旧路径由前端传入
pub(crate) async fn git_commit_file_blob(
    path: String,
    hash: String,
    file_path: String,
    parent: Option<bool>,
) -> AppResult<Option<String>> {
    run_blocking(move || {
        commit_file_blob_blocking(&path, &hash, &file_path, parent.unwrap_or(false))
    })
    .await
}

pub(crate) fn commit_file_blob_blocking(
    path: &str,
    hash: &str,
    file_path: &str,
    parent: bool,
) -> AppResult<Option<String>> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let commit = repo
        .revparse_single(hash)
        .and_then(|o| o.peel_to_commit())
        .map_err(git_err)?;
    let commit = if parent {
        // 根提交无父提交:文件在旧版本必然不存在,视作 None(与新增文件同语义)
        match commit.parent(0) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        }
    } else {
        commit
    };
    let tree = commit.tree().map_err(git_err)?;
    let Ok(entry) = tree.get_path(Path::new(file_path)) else {
        return Ok(None);
    };
    if entry.kind() != Some(git2::ObjectType::Blob) {
        return Ok(None);
    }
    let blob = repo.find_blob(entry.id()).map_err(git_err)?;
    if blob.size() > COMMIT_BLOB_MAX_BYTES {
        return Err(AppError::coded(
            ErrorCode::GitCommandFailed,
            format!("blob too large: {} bytes", blob.size()),
        ));
    }
    use base64::Engine as _;
    Ok(Some(
        base64::engine::general_purpose::STANDARD.encode(blob.content()),
    ))
}
