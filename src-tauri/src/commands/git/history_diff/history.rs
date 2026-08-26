use super::super::*;

/// 读取提交记录(日报生成用),按时间倒序。
/// author 传入时按 git --author 语义过滤(匹配 "Name <email>")。
/// 非 git 仓库或尚无提交时返回空数组而非报错(多项目汇总时容错)
pub(crate) async fn git_log(
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
pub(crate) async fn git_graph_log(
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
pub(crate) fn build_graph_revwalk<'r>(
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
pub(crate) struct GraphDeco {
    by_oid: HashMap<git2::Oid, Vec<String>>,
    head_oid: Option<git2::Oid>,
    head_branch: Option<String>,
}

impl GraphDeco {
    pub(crate) fn collect(repo: &Repository) -> Self {
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
    pub(crate) fn commit_entry(&self, commit: &git2::Commit) -> GitGraphCommit {
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
