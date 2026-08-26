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
    let recent_commits = (|| -> Option<Vec<String>> {
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
            if let Some(s) = commit.summary() {
                out.push(s.to_string());
            }
            if out.len() >= RECENT_COMMITS_COUNT {
                break;
            }
        }
        Some(out)
    })()
    .unwrap_or_default();

    Ok(GitCommitContext {
        stat,
        diff: diff_text,
        truncated,
        untracked,
        untracked_files,
        recent_commits,
    })
}
