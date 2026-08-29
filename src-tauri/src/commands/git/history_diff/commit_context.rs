use super::{super::*, truncate_chars};

/// 送入 AI 的 diff 长度上限(超出截断,避免 token 爆炸)
const DIFF_MAX_CHARS: usize = 30_000;
/// 单个未跟踪文件内容上限(字符)
const UNTRACKED_FILE_MAX_CHARS: usize = 4_000;
/// 全部未跟踪文件内容的总预算(字符)
const UNTRACKED_TOTAL_MAX_CHARS: usize = 12_000;
/// 二进制嗅探的前缀长度(含 NUL 即视为二进制)
const BINARY_SNIFF_BYTES: usize = 8_000;
/// 风格锚定用的最近提交条数
const RECENT_COMMITS_COUNT: usize = 10;

/// diff 噪声文件:内容对撰写提交信息无意义,排除以节省 token 预算。
/// pathspec 的 `*` 可跨目录匹配,无需逐层列举;stat 仍保留这些文件(摘要成本低且"锁文件变了"本身有价值)
const DIFF_EXCLUDES: &[&str] = &[
    ":(exclude)*pnpm-lock.yaml",
    ":(exclude)*package-lock.json",
    ":(exclude)*yarn.lock",
    ":(exclude)*bun.lockb",
    ":(exclude)*Cargo.lock",
    ":(exclude)*.min.js",
    ":(exclude)*.min.css",
    ":(exclude)*.map",
];

/// diff 噪声文件判断:pathspec `*` 可跨目录匹配,等价于按后缀匹配文件名。
/// stat 仍保留这些文件(摘要成本低且"锁文件变了"本身有价值)
fn is_diff_excluded(path: &str) -> bool {
    DIFF_EXCLUDES.iter().any(|p| {
        p.strip_prefix(":(exclude)*")
            .is_some_and(|suffix| path.ends_with(suffix))
    })
}

/// 读取未跟踪新文件的文本内容;非常规文件/二进制/读失败返回 None(由调用方回退到仅列文件名)
fn read_untracked_file(repo: &str, rel: &str) -> Option<GitUntrackedFile> {
    let full = Path::new(repo).join(rel);
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() {
        return None;
    }
    // 按字节多读一段用于二进制嗅探;char 截断在解码后做(UTF-8 最多 4 字节/字符,预算留足)
    let max_bytes = (UNTRACKED_FILE_MAX_CHARS * 4 + BINARY_SNIFF_BYTES) as u64;
    let mut buf = Vec::new();
    std::fs::File::open(&full)
        .ok()?
        .take(max_bytes)
        .read_to_end(&mut buf)
        .ok()?;
    if buf[..buf.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let (content, char_truncated) = truncate_chars(&text, UNTRACKED_FILE_MAX_CHARS);
    Some(GitUntrackedFile {
        path: rel.to_string(),
        content,
        truncated: char_truncated || meta.len() > buf.len() as u64,
    })
}

/// 收集 AI 生成提交信息所需的变更上下文:
/// 覆盖已暂存 + 已跟踪未暂存修改(与 git_commit 语义一致,相对 HEAD);
/// 仓库尚无提交(无 HEAD)时回退到暂存区 diff;
/// diff 排除锁文件/min/map 等噪声文件(stat 保留);
/// 未跟踪清单剔除嵌套 git 仓库目录(子仓库是独立项目,不算本仓库内容),
/// 其中可读的文本文件附带内容(预算受限,二进制跳过);
/// 附最近若干条提交 subject 供模型对齐仓库提交风格
pub(crate) async fn git_commit_context(path: String) -> AppResult<GitCommitContext> {
    run_blocking(move || commit_context_blocking(&path)).await
}

pub(crate) fn commit_context_blocking(path: &str) -> AppResult<GitCommitContext> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };

    // diff:相对 HEAD(等价 git diff HEAD,覆盖已暂存+已跟踪未暂存修改,与 git_commit 语义一致);
    // 仓库尚无提交(无 HEAD)时回退到暂存区 diff(相对空树,等价 git diff --cached)
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = match &head_tree {
        Some(tree) => repo.diff_tree_to_workdir_with_index(Some(tree), None),
        None => {
            let index = repo.index().map_err(git_err)?;
            repo.diff_tree_to_index(None, Some(&index), None)
        }
    }
    .map_err(git_err)?;

    // stat 全量保留(含锁文件等噪声文件);
    // 空 diff 时 libgit2 也会输出 "0 files changed..." 摘要行,与 git --stat 对齐为空串
    let stat = if diff.deltas().len() == 0 {
        String::new()
    } else {
        diff.stats()
            .and_then(|s| {
                s.to_buf(
                    git2::DiffStatsFormat::FULL | git2::DiffStatsFormat::INCLUDE_SUMMARY,
                    80,
                )
            })
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .unwrap_or_default()
    };

    // diff 文本排除锁文件/min/map 等噪声文件(节省 token 预算)
    let mut diff_text = String::new();
    for (idx, delta) in diff.deltas().enumerate() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if is_diff_excluded(&path) {
            continue;
        }
        if let Ok(Some(mut patch)) = Patch::from_diff(&diff, idx) {
            if let Ok(buf) = patch.to_buf() {
                diff_text.push_str(&String::from_utf8_lossy(&buf));
            }
        }
    }
    let (diff_text, truncated) = truncate_chars(&diff_text, DIFF_MAX_CHARS);

    // 未跟踪清单(等价 ls-files --others --exclude-standard:递归目录、不含忽略文件),
    // 剔除嵌套 git 仓库目录(子仓库是独立项目,不算本仓库内容)
    let mut sopts = StatusOptions::new();
    sopts
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut sopts)).map_err(git_err)?;
    let workdir = repo
        .workdir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    // 缓存本次扫描中已确认的嵌套仓库
    let mut nested_cache: HashSet<String> = HashSet::new();
    let untracked: Vec<String> = statuses
        .iter()
        .filter(|e| e.status().contains(Git2Status::WT_NEW))
        .map(|e| String::from_utf8_lossy(e.path_bytes()).to_string())
        .filter(|p| !is_nested_repo_cached(&workdir, p, &mut nested_cache))
        .collect();

    let mut untracked_files = Vec::new();
    let mut budget = UNTRACKED_TOTAL_MAX_CHARS;
    for name in &untracked {
        if budget == 0 {
            break;
        }
        if let Some(mut f) = read_untracked_file(&workdir, name) {
            let (content, hit_budget) = truncate_chars(&f.content, budget);
            f.truncated = f.truncated || hit_budget;
            budget -= content.chars().count();
            f.content = content;
            untracked_files.push(f);
        }
    }

    // 最近若干条提交 subject(无合并),供模型对齐仓库提交风格
    let recent_commits = recent_commit_messages(&repo);

    Ok(GitCommitContext {
        stat,
        diff: diff_text,
        truncated,
        untracked,
        untracked_files,
        recent_commits,
    })
}
fn recent_commit_messages(repo: &Repository) -> Vec<String> {
    (|| -> Option<Vec<String>> {
        let mut walk = repo.revwalk().ok()?;
        walk.push_head().ok()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME).ok()?;
        let mut out = Vec::new();
        for oid in walk.flatten() {
            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };
            if commit.parent_count() >= 2 {
                continue;
            }
            if let Some(summary) = commit.summary() {
                out.push(summary.to_string());
            }
            if out.len() >= RECENT_COMMITS_COUNT {
                break;
            }
        }
        Some(out)
    })()
    .unwrap_or_default()
}

/// AI 提交信息使用的单文件上下文。raw_patch 只会在 sem 未覆盖该文件或 sem 失败时进入提示词。
#[derive(Debug, Clone)]
pub(crate) struct AiCommitFileContext {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub raw_patch: String,
    pub binary: bool,
    pub raw_excluded: bool,
}

/// 与本次真实提交范围一致的 AI 上下文；semantic_input 仅在本地交给 sem，不直接发送给模型。
#[derive(Debug)]
pub(crate) struct AiCommitContext {
    pub stat: String,
    pub semantic_input: String,
    pub files: Vec<AiCommitFileContext>,
    pub recent_commits: Vec<String>,
}

pub(crate) async fn ai_commit_context(
    path: String,
    include_untracked: bool,
    paths: Option<Vec<String>>,
) -> AppResult<AiCommitContext> {
    run_blocking(move || ai_commit_context_blocking(&path, include_untracked, paths.as_deref()))
        .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SemFileChangeInput {
    file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_file_path: Option<String>,
    status: String,
    before_content: Option<String>,
    after_content: Option<String>,
}

fn blob_text(repo: &Repository, oid: git2::Oid) -> Option<String> {
    let blob = repo.find_blob(oid).ok()?;
    if blob.content().contains(&0) {
        return None;
    }
    std::str::from_utf8(blob.content()).ok().map(str::to_string)
}

fn workdir_text(workdir: &str, path: &str) -> Option<String> {
    let bytes = std::fs::read(Path::new(workdir).join(path)).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
fn binary_patch(path: &str, old_path: Option<&str>, status: &str) -> String {
    let old = old_path.unwrap_or(path);
    match status {
        "A" => format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n"
        ),
        "D" => format!(
            "diff --git a/{path} b/{path}\ndeleted file mode 100644\nBinary files a/{path} and /dev/null differ\n"
        ),
        _ => format!(
            "diff --git a/{old} b/{path}\nBinary files a/{old} and b/{path} differ\n"
        ),
    }
}
pub(crate) fn ai_commit_context_blocking(
    path: &str,
    include_untracked: bool,
    paths: Option<&[String]>,
) -> AppResult<AiCommitContext> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    if paths.is_some_and(<[String]>::is_empty) {
        return Err(AppError::coded(ErrorCode::GitPathsRequired, ""));
    }

    let normalized_paths = paths.map(|items| {
        items
            .iter()
            .map(|item| crate::path_util::to_forward_slash_str(item))
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
    });
    let diff = super::worktree_changes::worktree_diff(&repo, |opts| {
        opts.include_untracked(include_untracked)
            .recurse_untracked_dirs(include_untracked)
            .show_untracked_content(include_untracked);
        if let Some(items) = &normalized_paths {
            for item in items {
                opts.pathspec(item);
            }
        }
    })?;
    let workdir = repo
        .workdir()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    let mut nested_cache = HashSet::new();
    let mut semantic_inputs = Vec::new();
    let mut files = Vec::new();

    for (idx, delta) in diff.deltas().enumerate() {
        let status = match delta.status() {
            Delta::Added | Delta::Untracked => "A",
            Delta::Copied => "C",
            Delta::Deleted => "D",
            Delta::Modified => "M",
            Delta::Renamed => "R",
            Delta::Typechange => "T",
            _ => continue,
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|value| crate::path_util::to_forward_slash_str(&value.to_string_lossy()))
            .unwrap_or_default();
        if path.is_empty() || is_nested_repo_cached(&workdir, &path, &mut nested_cache) {
            continue;
        }
        let old_path = if matches!(delta.status(), Delta::Renamed | Delta::Copied) {
            delta
                .old_file()
                .path()
                .map(|value| crate::path_util::to_forward_slash_str(&value.to_string_lossy()))
        } else {
            None
        };
        let patch = Patch::from_diff(&diff, idx).ok().flatten();
        let binary = diff
            .get_delta(idx)
            .map(|value| value.flags().is_binary())
            .unwrap_or(false);
        let mut raw_patch = if binary {
            binary_patch(&path, old_path.as_deref(), status)
        } else {
            patch
                .and_then(|mut value| value.to_buf().ok())
                .map(|value| String::from_utf8_lossy(&value).into_owned())
                .unwrap_or_default()
        };
        if raw_patch.trim().is_empty() {
            let old_label = old_path.as_deref().unwrap_or(&path);
            raw_patch = format!("diff --git a/{old_label} b/{path}\n{status} {path}\n");
        }
        let sem_status = match status {
            "A" | "C" => "added",
            "D" => "deleted",
            "R" => "renamed",
            _ => "modified",
        };
        let before_content = if binary || matches!(status, "A" | "C") {
            None
        } else {
            blob_text(&repo, delta.old_file().id())
        };
        let after_content = if binary || status == "D" {
            None
        } else {
            workdir_text(&workdir, &path)
        };
        semantic_inputs.push(SemFileChangeInput {
            file_path: path.clone(),
            old_file_path: if status == "R" {
                old_path.clone()
            } else {
                None
            },
            status: sem_status.to_string(),
            before_content,
            after_content,
        });
        files.push(AiCommitFileContext {
            path: path.clone(),
            old_path,
            status: status.to_string(),
            raw_patch,
            binary,
            raw_excluded: is_diff_excluded(&path),
        });
    }

    let stat = if files.is_empty() {
        String::new()
    } else {
        files
            .iter()
            .map(|file| {
                let kind = if file.binary {
                    "binary"
                } else {
                    file.status.as_str()
                };
                format!("{} | {kind}", file.path)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let semantic_input = serde_json::to_string(&semantic_inputs)
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolFailed, error.to_string()))?;
    Ok(AiCommitContext {
        stat,
        semantic_input,
        files,
        recent_commits: recent_commit_messages(&repo),
    })
}
