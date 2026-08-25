use super::*;

// ── worktree / merge / rebase ─────────────────────────────

/// 从已打开的仓库读 worktree 展示信息(主工作区或链接 worktree 通用)。
/// 路径统一为 '/' 分隔(与原 `git worktree list --porcelain` 在 Windows 上的输出一致)
pub(super) fn worktree_info_of(wt_repo: &Repository, wt_path: &Path, is_main: bool) -> GitWorktree {
    let mut w = GitWorktree {
        path: crate::path_util::to_forward_slash(wt_path),
        branch: None,
        head: String::new(),
        is_main,
        detached: false,
        base_branch: None,
        base_behind: None,
    };
    if let Ok(head) = wt_repo.head() {
        w.head = head.target().map(|oid| oid.to_string()).unwrap_or_default();
        if head.is_branch() {
            w.branch = head.shorthand().map(String::from);
        } else {
            w.detached = true;
        }
    }
    w.base_branch = base_branch_of(wt_repo, w.branch.as_deref());
    w.base_behind = base_behind_of(wt_repo, w.base_branch.as_deref(), &w.head);
    w
}

/// 来源分支领先 HEAD 的提交数(>0 表示变基可带入新提交)。base 依次按本地分支、
/// 远程引用(origin/x)解析;无 base、HEAD 为空或引用已不存在时为 None。
/// graph_ahead_behind(base, head) 的 ahead 即 base 独有、变基会带入的提交数
pub(super) fn base_behind_of(repo: &Repository, base: Option<&str>, head: &str) -> Option<usize> {
    let base = base?;
    let head_oid = git2::Oid::from_str(head).ok()?;
    let base_oid = ["refs/heads/", "refs/remotes/"]
        .iter()
        .find_map(|prefix| repo.revparse_single(&format!("{prefix}{base}")).ok())
        .map(|obj| obj.id())?;
    repo.graph_ahead_behind(base_oid, head_oid)
        .ok()
        .map(|(ahead, _)| ahead)
}

/// 分支的创建来源:`branch.<name>.repomeow-base`(本应用新建 worktree 分支时记录);
/// 无记录时回退为上游跟踪分支,组装为 origin/x 形式(上游为本地分支 remote="." 时
/// 直接返回本地分支名)。游离 HEAD 或无上游时为 None
pub(super) fn base_branch_of(repo: &Repository, branch: Option<&str>) -> Option<String> {
    let name = branch?;
    let cfg = repo.config().ok()?;
    let key = format!("branch.{name}.repomeow-base");
    if let Ok(base) = cfg.get_string(&key) {
        let base = base.trim();
        if !base.is_empty() {
            return Some(base.to_string());
        }
    }
    let remote = cfg
        .get_string(&format!("branch.{name}.remote"))
        .ok()?
        .trim()
        .to_string();
    let merge = cfg
        .get_string(&format!("branch.{name}.merge"))
        .ok()?
        .trim()
        .to_string();
    let short = merge.strip_prefix("refs/heads/").unwrap_or(&merge);
    if short.is_empty() {
        return None;
    }
    if remote.is_empty() || remote == "." {
        Some(short.to_string())
    } else {
        Some(format!("{remote}/{short}"))
    }
}

/// 归一到主工作区句柄。path 可能是链接工作区本身(worktree 副本会登记为独立项目),
/// 该视角下 workdir() 是副本自身、worktrees() 只列链接工作区,主工作区会整体缺失
/// 且 is_main 标错对象。统一换成主工作区视角再列举;推导或打开失败
/// (separate-git-dir、子模块等非标准布局)时原样返回,退回传入路径视角
pub(super) fn main_worktree_repo(repo: Repository) -> Repository {
    if repo.path() == repo.commondir() {
        return repo;
    }
    let Some(main_root) = main_worktree_root(&repo) else {
        return repo;
    };
    let root_str = main_root.to_string_lossy();
    match open_repo(&root_str) {
        Ok(Some(main)) => main,
        _ => repo,
    }
}

/// 主工作区根目录:commondir(标准布局为 `<主工作区>/.git`)的上一级,与
/// `git worktree list` 的推导一致;末段不是 .git 时无法推导,返回 None
pub(super) fn main_worktree_root(repo: &Repository) -> Option<PathBuf> {
    let commondir = repo.commondir();
    if !commondir.file_name()?.eq_ignore_ascii_case(".git") {
        return None;
    }
    commondir.parent().map(PathBuf::from)
}

pub(super) fn list_worktrees_blocking(path: &str) -> AppResult<Vec<GitWorktree>> {
    let Some(repo) = open_repo(path)? else {
        return Err(not_a_repo());
    };
    let repo = main_worktree_repo(repo);
    let mut list = Vec::new();
    // 主工作区始终第一条
    if let Some(workdir) = repo.workdir() {
        list.push(worktree_info_of(&repo, workdir, true));
    }
    let names = repo.worktrees().map_err(git_err)?;
    for name in names.iter().flatten() {
        let Ok(wt) = repo.find_worktree(name) else {
            continue;
        };
        let wt_path = wt.path().to_path_buf();
        // 打开 worktree 读其 HEAD(.git 为 gitfile,libgit2 自动解析);
        // 打开失败(目录已被手工删除等)仍列出,保留路径供移除
        match Repository::open(&wt_path) {
            Ok(wt_repo) => list.push(worktree_info_of(&wt_repo, &wt_path, false)),
            Err(_) => list.push(GitWorktree {
                path: crate::path_util::to_forward_slash(&wt_path),
                branch: None,
                head: String::new(),
                is_main: false,
                detached: false,
                base_branch: None,
                base_behind: None,
            }),
        }
    }
    // 工作区内的 worktree 目录排除出未跟踪(新建与存量都经此处自愈)
    if let Some(workdir) = repo.workdir() {
        let commondir = repo.commondir().to_path_buf();
        for w in list.iter().skip(1) {
            ensure_worktree_excluded(&commondir, workdir, Path::new(&w.path));
        }
    }
    Ok(list)
}

/// 位于仓库工作区内的 worktree 目录(默认 .worktrees/{branch})会让所在仓库 status
/// 多出一条未跟踪目录,误 `git add -A` 还会被加成 embedded gitlink;写入 .git/info/exclude
/// 本地排除(不动可能被跟踪的 .gitignore)。幂等:已有相同行则跳过,读写失败静默忽略
pub(super) fn ensure_worktree_excluded(commondir: &Path, workdir: &Path, worktree_path: &Path) {
    let Ok(rel) = worktree_path.strip_prefix(workdir) else {
        return; // 工作区外的 worktree 不影响本仓库状态
    };
    let rel = crate::path_util::to_forward_slash(rel);
    if rel.is_empty() {
        return;
    }
    let line = format!("/{rel}/");
    let info_dir = commondir.join("info");
    let exclude = info_dir.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == line) {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&info_dir) {
        eprintln!(
            "[git] 写入 worktree 排除失败(创建 {}): {e}",
            info_dir.display()
        );
        return;
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("# RepoMeow worktree\n");
    content.push_str(&line);
    content.push('\n');
    if let Err(e) = std::fs::write(&exclude, content) {
        eprintln!("[git] 写入 worktree 排除失败({}): {e}", exclude.display());
    }
}

#[tauri::command]
pub async fn list_git_worktrees(path: String) -> AppResult<Vec<GitWorktree>> {
    run_blocking(move || list_worktrees_blocking(&path)).await
}

/// worktree 目标目录:`{branch}` 占位符替换为分支名(`/` 转 `-`,避免多级路径);
/// 相对路径基于主工作区根解析,绝对路径原样使用
pub(super) fn resolve_worktree_target(main_root: &str, input: &str, branch: &str) -> PathBuf {
    let templated = input.replace("{branch}", &branch.replace('/', "-"));
    let p = Path::new(&templated);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(main_root).join(p)
    }
}

/// 创建 worktree。create_branch 为 true 时检出新分支
/// (`git worktree add <dir> -b <branch> [start_point]`,start_point 缺省为 HEAD),
/// 并把 base_branch(创建来源分支)写入 `branch.<branch>.repomeow-base` 配置;
/// 为 false 时挂载已有分支:本地分支直接挂载;origin/xxx 远程引用在本地无同名分支时
/// 显式创建跟踪分支(直接传 origin/x 只会得到游离 HEAD,不触发 checkout 式 DWIM),
/// 本地已有同名分支时按安全快进对齐(见 attach_remote_worktree)。
/// 分支已被其它 worktree 检出时报 git_branch_checked_out
#[tauri::command]
pub async fn git_worktree_add(
    app: AppHandle,
    path: String,
    worktree_path: String,
    branch: String,
    create_branch: bool,
    start_point: Option<String>,
    base_branch: Option<String>,
) -> AppResult<Vec<GitWorktree>> {
    let event_path = path.clone();
    let worktrees = run_blocking(move || {
        worktree_add_blocking(
            &path,
            &worktree_path,
            &branch,
            create_branch,
            start_point.as_deref(),
            base_branch.as_deref(),
        )
    })
    .await?;
    if let Ok(status) = run_blocking({
        let path = event_path.clone();
        move || status_cached(&path, true)
    })
    .await
    {
        publish_write_status(&app, &event_path, &status, "worktree_add", false);
    }
    Ok(worktrees)
}

pub(super) fn worktree_add_blocking(
    path: &str,
    worktree_path: &str,
    branch: &str,
    create_branch: bool,
    start_point: Option<&str>,
    base_branch: Option<&str>,
) -> AppResult<Vec<GitWorktree>> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    let input = worktree_path.trim();
    if input.is_empty() {
        return Err(AppError::coded(ErrorCode::InvalidPath, ""));
    }
    let existing = list_worktrees_blocking(path)?;
    // porcelain 第一条即主工作区;查不到(异常)时退回传入路径
    let main_root = existing
        .first()
        .map(|w| w.path.clone())
        .unwrap_or_else(|| path.to_string());
    let locals = local_branch_names(path)?;
    let is_local = locals.iter().any(|b| b == branch);
    // 挂载已有分支时,远程引用(origin/x)落地后的本地名是去掉首段前缀的部分
    let local_name = if create_branch || is_local {
        branch
    } else {
        branch.split_once('/').map(|(_, s)| s).unwrap_or(branch)
    };
    if existing
        .iter()
        .any(|w| w.branch.as_deref() == Some(local_name))
    {
        return Err(AppError::coded(ErrorCode::GitBranchCheckedOut, local_name));
    }
    let target = resolve_worktree_target(&main_root, input, branch);
    let target_str = target.to_string_lossy().to_string();
    if create_branch {
        if locals.iter().any(|b| b == branch) {
            return Err(AppError::coded(ErrorCode::GitBranchExists, branch));
        }
        match start_point.map(str::trim).filter(|s| !s.is_empty()) {
            Some(base) => run_git(path, &["worktree", "add", &target_str, "-b", branch, base])?,
            None => run_git(path, &["worktree", "add", &target_str, "-b", branch])?,
        };
    } else if is_local {
        run_git(path, &["worktree", "add", &target_str, branch])?;
    } else {
        attach_remote_worktree(path, &target_str, branch, local_name, &locals)?;
    }
    // 新建分支时记录创建来源(git config branch.<name>.repomeow-base),
    // 供合并/变基默认回到来源分支;删分支时 git 会连带清掉该段配置。
    // 配置写入失败不阻断创建(仅丢失来源记录)
    if create_branch {
        if let Some(base) = base_branch.map(str::trim).filter(|s| !s.is_empty()) {
            let key = format!("branch.{branch}.repomeow-base");
            if let Err(e) = run_git(path, &["config", &key, base]) {
                eprintln!("[git] 记录 worktree 来源分支失败({key}={base}): {e}");
            }
        }
    }
    list_worktrees_blocking(path)
}

/// 挂载远程引用(origin/x)到 worktree。
/// - 本地无同名分支:`git worktree add --track -b x <dir> origin/x` 显式建跟踪分支
/// - 本地已有同名分支且可能不同步:本地落后/持平时先 `git branch -f` 对齐到远程提交
///   再挂载;本地领先(远程是其祖先)直接挂载本地分支;真正分叉时报
///   git_branch_diverged —— 不静默重置分支,避免本地未推送提交从分支上丢失
pub(super) fn attach_remote_worktree(
    path: &str,
    target: &str,
    remote: &str,
    local_name: &str,
    locals: &[String],
) -> AppResult<()> {
    if locals.iter().any(|b| b == local_name) {
        if is_ancestor(path, local_name, remote)? {
            // 落后或持平:对齐到远程提交(持平为 no-op);此时本地分支未被任何
            // worktree 检出(前置已查),branch -f 安全
            run_git(path, &["branch", "-f", local_name, remote])?;
        } else if !is_ancestor(path, remote, local_name)? {
            return Err(AppError::coded(ErrorCode::GitBranchDiverged, local_name));
        }
        run_git(path, &["worktree", "add", target, local_name])?;
    } else {
        run_git(
            path,
            &[
                "worktree", "add", "--track", "-b", local_name, target, remote,
            ],
        )?;
    }
    Ok(())
}

/// `git merge-base --is-ancestor a b`:a 是否为 b 的祖先(0=是,1=否,其它视为命令失败)
pub(super) fn is_ancestor(path: &str, a: &str, b: &str) -> AppResult<bool> {
    let out = git_command(path)
        .args(["merge-base", "--is-ancestor", a, b])
        .output()?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(AppError::coded(
                ErrorCode::GitCommandFailed,
                format!("merge-base --is-ancestor {a} {b}: {stderr}"),
            ))
        }
    }
}
/// 删除 worktree(`git worktree remove [--force]`),可选同时删除其检出的本地分支
/// (force 时用 -D,否则 -d 安全删除)。主工作区不可删除。返回最新 worktree 列表。
/// 幂等可重试:worktree 登记或目录已不存在(目录被外部删除,或上次删 worktree 成功但
/// 分支 -d 因未合并失败后的强制重试)时改为 prune 清理登记,并按 branch 继续删分支
#[tauri::command]
pub async fn git_worktree_remove(
    app: AppHandle,
    path: String,
    worktree_path: String,
    force: bool,
    delete_branch: bool,
    branch: Option<String>,
) -> AppResult<Vec<GitWorktree>> {
    let event_path = path.clone();
    let worktrees = run_blocking(move || {
        worktree_remove_blocking(
            &path,
            &worktree_path,
            force,
            delete_branch,
            branch.as_deref(),
        )
    })
    .await?;
    if let Ok(status) = run_blocking({
        let path = event_path.clone();
        move || status_cached(&path, true)
    })
    .await
    {
        publish_write_status(&app, &event_path, &status, "worktree_remove", false);
    }
    Ok(worktrees)
}

pub(super) fn worktree_remove_blocking(
    path: &str,
    worktree_path: &str,
    force: bool,
    delete_branch: bool,
    branch_hint: Option<&str>,
) -> AppResult<Vec<GitWorktree>> {
    let existing = list_worktrees_blocking(path)?;
    let target = existing.iter().find(|w| w.path == worktree_path);
    if let Some(t) = target {
        if t.is_main {
            return Err(AppError::coded(
                ErrorCode::GitCommandFailed,
                "cannot remove main worktree",
            ));
        }
    }
    // 分支名优先取 worktree 登记;登记已不在(上次删 worktree 成功、分支未删的重试)
    // 时用前端传入的候选名
    let branch = target.and_then(|t| t.branch.clone()).or_else(|| {
        branch_hint
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    if target.is_some() && Path::new(worktree_path).exists() {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(worktree_path);
        run_git(path, &args)?;
    } else {
        // 登记或目录已缺失:`git worktree remove` 会因缺少 `.git` 先验校验失败,
        // 交给 prune 清理悬挂登记,避免把可恢复的清理/重试报成删除失败
        run_git(path, &["worktree", "prune"])?;
    }
    if delete_branch {
        if let Some(b) = branch {
            let flag = if force { "-D" } else { "-d" };
            run_git(path, &["branch", flag, &b])?;
        }
    }
    list_worktrees_blocking(path)
}
