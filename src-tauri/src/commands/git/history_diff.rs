use super::*;

/// 送入 AI 的 diff 长度上限(超出截断,避免 token 爆炸)
pub(super) const DIFF_MAX_CHARS: usize = 30_000;
/// 单个未跟踪文件内容上限(字符)
pub(super) const UNTRACKED_FILE_MAX_CHARS: usize = 4_000;
/// 全部未跟踪文件内容的总预算(字符)
pub(super) const UNTRACKED_TOTAL_MAX_CHARS: usize = 12_000;
/// 二进制嗅探的前缀长度(含 NUL 即视为二进制)
pub(super) const BINARY_SNIFF_BYTES: usize = 8_000;
/// 风格锚定用的最近提交条数
pub(super) const RECENT_COMMITS_COUNT: usize = 10;

/// diff 噪声文件:内容对撰写提交信息无意义,排除以节省 token 预算。
/// pathspec 的 `*` 可跨目录匹配,无需逐层列举;stat 仍保留这些文件(摘要成本低且"锁文件变了"本身有价值)
pub(super) const DIFF_EXCLUDES: &[&str] = &[
    ":(exclude)*pnpm-lock.yaml",
    ":(exclude)*package-lock.json",
    ":(exclude)*yarn.lock",
    ":(exclude)*bun.lockb",
    ":(exclude)*Cargo.lock",
    ":(exclude)*.min.js",
    ":(exclude)*.min.css",
    ":(exclude)*.map",
];

/// 按 char 边界安全截断,返回 (文本, 是否截断)
pub(super) fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let end = text
        .char_indices()
        .nth(max)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    (text[..end].to_string(), true)
}

/// diff 噪声文件判断:pathspec `*` 可跨目录匹配,等价于按后缀匹配文件名。
/// stat 仍保留这些文件(摘要成本低且"锁文件变了"本身有价值)
pub(super) fn is_diff_excluded(path: &str) -> bool {
    DIFF_EXCLUDES.iter().any(|p| {
        p.strip_prefix(":(exclude)*")
            .is_some_and(|suffix| path.ends_with(suffix))
    })
}

/// 读取未跟踪新文件的文本内容;非常规文件/二进制/读失败返回 None(由调用方回退到仅列文件名)
pub(super) fn read_untracked_file(repo: &str, rel: &str) -> Option<GitUntrackedFile> {
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
#[tauri::command]
pub async fn git_commit_context(path: String) -> AppResult<GitCommitContext> {
    run_blocking(move || commit_context_blocking(&path)).await
}

pub(super) fn commit_context_blocking(path: &str) -> AppResult<GitCommitContext> {
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
    let mut nested_cache: std::collections::HashSet<String> = std::collections::HashSet::new();
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

/// 读取提交记录(日报生成用),按时间倒序。
/// author 传入时按 git --author 语义过滤(匹配 "Name <email>")。
/// 非 git 仓库或尚无提交时返回空数组而非报错(多项目汇总时容错)
#[tauri::command]
pub async fn git_log(
    path: String,
    since: Option<String>,
    until: Option<String>,
    max_count: Option<u32>,
    author: Option<String>,
) -> AppResult<Vec<GitCommitInfo>> {
    run_blocking(move || {
        run_git_log(
            &path,
            since.as_deref(),
            until.as_deref(),
            max_count,
            author.as_deref(),
        )
    })
    .await
}

/// git_log 核心逻辑,供 scheduler 等内部模块复用;参数均为引用以避免不必要的 clone。
/// 非 git 仓库或尚无提交时返回空数组而非报错(多项目汇总时容错)
pub(crate) fn run_git_log(
    path: &str,
    since: Option<&str>,
    until: Option<&str>,
    max_count: Option<u32>,
    author: Option<&str>,
) -> AppResult<Vec<GitCommitInfo>> {
    let Some(repo) = open_repo(path)? else {
        return Ok(Vec::new());
    };
    let mut walk = repo
        .revwalk()
        .map_err(|e| AppError::coded(ErrorCode::GitLogFailed, e.to_string()))?;
    // 尚无提交(未出生 HEAD)→ 空
    if walk.push_head().is_err() {
        return Ok(Vec::new());
    }
    // git log 默认按提交时间倒序;加 TOPOLOGICAL 保证同秒时间戳下父提交不会跑到
    // 子提交前面(libgit2 的时间堆在完全相等时不保证稳定,线性历史也观察到乱序)
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|e| AppError::coded(ErrorCode::GitLogFailed, e.to_string()))?;

    let since_ts = since
        .filter(|s| !s.trim().is_empty())
        .and_then(parse_log_datetime);
    let until_ts = until
        .filter(|s| !s.trim().is_empty())
        .and_then(parse_log_datetime);
    let author = author.map(str::trim).filter(|a| !a.is_empty());
    let limit = max_count.unwrap_or(200).min(1000) as usize;

    let mut commits = Vec::new();
    for oid in walk.flatten() {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        // --no-merges
        if commit.parent_count() >= 2 {
            continue;
        }
        // --since/--until 比较的是提交(committer)时间
        let committed = commit.time().seconds();
        if since_ts.is_some_and(|s| committed < s) || until_ts.is_some_and(|u| committed > u) {
            continue;
        }
        let author_sig = commit.author();
        let author_name = author_sig.name().unwrap_or_default();
        // --author 语义:匹配 "Name <email>"(git 为正则,这里用包含匹配覆盖常规用法)
        if let Some(a) = author {
            let full = format!(
                "{} <{}>",
                author_name,
                author_sig.email().unwrap_or_default()
            );
            if !full.contains(a) {
                continue;
            }
        }
        commits.push(GitCommitInfo {
            hash: short_hash(&commit),
            author: author_name.to_string(),
            date: format_git_time(author_sig.when()),
            subject: commit.summary().unwrap_or_default().to_string(),
        });
        if commits.len() >= limit {
            break;
        }
    }
    Ok(commits)
}

/// 读取提交图谱数据(含合并提交与引用装饰),按拓扑序流式输出,支持全量历史。
/// 拓扑排序保证子提交先于父提交,是前端泳道布局的前提;
/// 非 git 仓库或尚无提交时仅推送一个 done 批次而非报错。
/// 修订范围:branches 非空时按指定分支(本地或 origin/xxx)取日志;
/// 否则 include_remote 为 false 时仅本地分支+标签(refs/heads + refs/tags),默认全量引用。
/// 结果按 batch_size 分批经 channel 推送,最后一批 done = true
#[tauri::command]
pub async fn git_graph_log(
    path: String,
    branches: Option<Vec<String>>,
    include_remote: Option<bool>,
    batch_size: Option<u32>,
    on_batch: Channel<GitGraphBatch>,
) -> AppResult<()> {
    run_blocking(move || {
        let size = batch_size.unwrap_or(500).clamp(50, 2000) as usize;
        let Some(repo) = open_repo(&path)? else {
            let _ = on_batch.send(GitGraphBatch {
                commits: Vec::new(),
                done: true,
            });
            return Ok(());
        };
        let walk = match build_graph_revwalk(&repo, branches, include_remote)? {
            Some(w) => w,
            // 空修订范围/空仓库:仅推 done 批次
            None => {
                let _ = on_batch.send(GitGraphBatch {
                    commits: Vec::new(),
                    done: true,
                });
                return Ok(());
            }
        };
        let deco = GraphDeco::collect(&repo);
        let mut batch: Vec<GitGraphCommit> = Vec::with_capacity(size);
        for oid in walk.flatten() {
            let Ok(commit) = repo.find_commit(oid) else {
                continue;
            };
            batch.push(deco.commit_entry(&commit));
            if batch.len() >= size {
                let _ = on_batch.send(GitGraphBatch {
                    commits: std::mem::take(&mut batch),
                    done: false,
                });
            }
        }
        let _ = on_batch.send(GitGraphBatch {
            commits: batch,
            done: true,
        });
        Ok(())
    })
    .await
}

/// 构建图谱 revwalk:返回 None 表示修订范围为空或仓库无提交(调用方推 done 批次)。
/// branches 中无法解析的修订名报 git_log_failed(与原 git log 的 bad revision 一致)
pub(super) fn build_graph_revwalk<'r>(
    repo: &'r Repository,
    branches: Option<Vec<String>>,
    include_remote: Option<bool>,
) -> AppResult<Option<git2::Revwalk<'r>>> {
    let walk_err = |e: git2::Error| AppError::coded(ErrorCode::GitLogFailed, e.to_string());
    let mut tips: Vec<git2::Oid> = Vec::new();
    match branches {
        Some(list) => {
            let revs: Vec<String> = list
                .into_iter()
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect();
            if revs.is_empty() {
                return Ok(None);
            }
            for b in &revs {
                let commit = repo
                    .revparse_single(b)
                    .and_then(|o| o.peel_to_commit())
                    .map_err(|_| {
                        AppError::coded(ErrorCode::GitLogFailed, format!("bad revision: {b}"))
                    })?;
                tips.push(commit.id());
            }
        }
        None => {
            let all = include_remote != Some(false);
            let iter = repo.references().map_err(walk_err)?;
            for r in iter.flatten() {
                let name = r.name().unwrap_or_default();
                let include = if all {
                    name.starts_with("refs/")
                } else {
                    name.starts_with("refs/heads/") || name.starts_with("refs/tags/")
                };
                if include {
                    if let Ok(c) = r.peel_to_commit() {
                        tips.push(c.id());
                    }
                }
            }
            if all {
                // --all 语义含 HEAD(detached 时 HEAD 可能不在任何 refs 下)
                if let Ok(c) = repo.head().and_then(|h| h.peel_to_commit()) {
                    tips.push(c.id());
                }
            }
        }
    }
    if tips.is_empty() {
        return Ok(None);
    }
    let mut walk = repo.revwalk().map_err(walk_err)?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(walk_err)?;
    for tip in tips {
        walk.push(tip).map_err(walk_err)?;
    }
    Ok(Some(walk))
}

/// 引用装饰映射(Oid → 名称列表 + HEAD 指向),命名与 `git log %D` 一致:
/// refs/heads/x → "x",refs/remotes/o/x → "o/x",refs/tags/t → "tag: t",
/// origin/HEAD 这类符号引用不显示
pub(super) struct GraphDeco {
    by_oid: HashMap<git2::Oid, Vec<String>>,
    head_oid: Option<git2::Oid>,
    head_branch: Option<String>,
}

impl GraphDeco {
    pub(super) fn collect(repo: &Repository) -> Self {
        let mut by_oid: HashMap<git2::Oid, Vec<String>> = HashMap::new();
        if let Ok(iter) = repo.references() {
            for r in iter.flatten() {
                if r.kind() == Some(git2::ReferenceType::Symbolic) {
                    continue;
                }
                let Some(name) = r.name() else { continue };
                let display = name
                    .strip_prefix("refs/heads/")
                    .map(String::from)
                    .or_else(|| name.strip_prefix("refs/remotes/").map(String::from))
                    .or_else(|| name.strip_prefix("refs/tags/").map(|t| format!("tag: {t}")))
                    .unwrap_or_else(|| name.to_string());
                if let Ok(c) = r.peel_to_commit() {
                    by_oid.entry(c.id()).or_default().push(display);
                }
            }
        }
        let head = repo.head().ok();
        let head_oid = head.as_ref().and_then(|h| h.target());
        let head_branch = head
            .as_ref()
            .filter(|h| h.is_branch())
            .and_then(|h| h.shorthand().map(String::from));
        GraphDeco {
            by_oid,
            head_oid,
            head_branch,
        }
    }

    /// 单条提交转图谱条目:HEAD 分支名在 refs 中置顶(等价 %D 的 "HEAD -> x"),
    /// detached 时仅置 is_head
    pub(super) fn commit_entry(&self, commit: &git2::Commit) -> GitGraphCommit {
        let is_head = self.head_oid == Some(commit.id());
        let mut refs = Vec::new();
        if is_head {
            if let Some(b) = &self.head_branch {
                refs.push(b.clone());
            }
        }
        if let Some(names) = self.by_oid.get(&commit.id()) {
            for n in names {
                // "HEAD -> x" 已消费的分支名不重复列出
                if is_head && self.head_branch.as_deref() == Some(n.as_str()) {
                    continue;
                }
                refs.push(n.clone());
            }
        }
        GitGraphCommit {
            hash: commit.id().to_string(),
            parents: commit.parent_ids().map(|p| p.to_string()).collect(),
            author: commit.author().name().unwrap_or_default().to_string(),
            date: format_git_time(commit.author().when()),
            subject: commit.summary().unwrap_or_default().to_string(),
            refs,
            is_head,
        }
    }
}

/// 提交详情面板单文件 diff 的长度上限(超出截断,避免大文件撑爆 IPC)
pub(super) const COMMIT_DIFF_MAX_CHARS: usize = 200_000;

/// 读取某次提交的树间 diff(详情面板文件清单与单文件 diff 共用)。
/// 根提交相对空树(等价 diff-tree --root);-M 重命名识别;
/// 合并提交(多父)无单 diff 语义,返回 None(与原 diff-tree 默认无输出一致)
pub(super) fn commit_diff<'r>(
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
#[tauri::command]
pub async fn git_commit_files(path: String, hash: String) -> AppResult<Vec<GitCommitFile>> {
    run_blocking(move || commit_files_blocking(&path, &hash)).await
}

pub(super) fn commit_files_blocking(path: &str, hash: &str) -> AppResult<Vec<GitCommitFile>> {
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

/// 单文件 diff 的展示选项(commit / worktree 单文件 diff 共用):
/// 全量上下文(完整文件内容,前端并排/逐行视图自行折叠未更改区间)+ 忽略空白差异。
/// context_lines 拉满:u32::MAX 会使 libgit2 的 hunk 边界计算溢出,
/// 产生 @@ -4,2- +4 @@ 畸形头且丢上下文;100k 已足够——行数超 10 万的文件
/// 体积必然超过 COMMIT_DIFF_MAX_CHARS 字符上限,会先被截断
pub(super) fn apply_display_opts(opts: &mut DiffOptions, ignore_ws: Option<&str>) {
    opts.context_lines(100_000);
    // 忽略空白差异:eol=仅行尾 / change=空白数量变化 / all=全部空白(对应 git 的 -b / -w 语义)
    match ignore_ws {
        Some("eol") => {
            opts.ignore_whitespace_eol(true);
        }
        Some("change") => {
            opts.ignore_whitespace_change(true);
        }
        Some("all") => {
            opts.ignore_whitespace(true);
        }
        _ => {}
    }
}

/// 读取某次提交中单个文件的 diff(提交详情面板用)。
/// 重命名时新旧路径都作为 pathspec 传入。超长按字符截断(二进制 diff 天然很短)
/// context_lines 拉满:前端并排/逐行视图均展示完整文件内容(未更改区间由前端折叠)
/// ignore_ws:前端"忽略空白差异"模式,None/"none" 不忽略
#[tauri::command]
pub async fn git_commit_file_diff(
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

pub(super) fn commit_file_diff_blocking(
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
pub(super) const COMMIT_BLOB_MAX_BYTES: usize = 32 * 1024 * 1024;

/// 读取某次提交(或其第一个父提交)中一个文件的 blob 内容,base64 编码返回
/// (提交详情面板拼 data: URL 预览图片)。文件在该版本不存在(如新增文件的旧版本、
/// 根提交请求父版本)返回 None;子模块等非 blob 条目同样返回 None。
/// parent = true 时读父提交版本(旧版),否则读该提交本身(新版);重命名的旧路径由前端传入
#[tauri::command]
pub async fn git_commit_file_blob(
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

pub(super) fn commit_file_blob_blocking(
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

/// 构建工作区相对 HEAD 的 diff(覆盖已暂存 + 已跟踪未暂存修改 + 未跟踪文件,与 git_commit 语义一致);
/// 仓库尚无提交(无 HEAD)时回退到暂存区 diff(相对空树);
/// include_untracked 使未跟踪文件以 Added delta 出现,补丁内容直接读工作区
pub(super) fn worktree_diff<'r>(
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
#[tauri::command]
pub async fn git_worktree_files(path: String) -> AppResult<Vec<GitWorktreeFile>> {
    run_blocking(move || worktree_files_blocking(&path)).await
}

pub(super) fn worktree_files_blocking(path: &str) -> AppResult<Vec<GitWorktreeFile>> {
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
#[tauri::command]
pub async fn git_worktree_file_diff(
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

pub(super) fn worktree_file_diff_blocking(
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

/// 读取仓库当前 git 用户身份(日报"仅自己"过滤用)。
/// `git config user.name/email` 本身含全局配置回退;非仓库或未配置时返回空串而非报错
#[tauri::command]
pub async fn git_current_user(path: String) -> AppResult<GitUser> {
    run_blocking(move || run_git_current_user(&path)).await
}

pub(crate) fn run_git_current_user(path: &str) -> AppResult<GitUser> {
    // 仓库配置(libgit2 自动合并 local/global/system);非仓库回退全局配置,
    // 与 `git config user.name` 在仓库外仍读全局的行为一致
    let cfg = open_repo(path)
        .ok()
        .flatten()
        .and_then(|r| r.config().ok())
        .or_else(|| git2::Config::open_default().ok());
    let read = |key: &str| -> String {
        cfg.as_ref()
            .and_then(|c| c.get_string(key).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    Ok(GitUser {
        name: read("user.name"),
        email: read("user.email"),
    })
}
