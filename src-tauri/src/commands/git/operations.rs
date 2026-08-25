use super::*;

/// 在项目目录初始化 git 仓库,返回最新状态。
/// branch 为初始分支名(空回退 main);`git init -b` 需要 git 2.28+,
/// 旧版本回退到不带 -b 的 init 后用 `checkout -b` 改名未出生分支。
/// remote_url 非空时将其绑定为 origin,重复调用时:
/// - origin 已存在 → `git remote set-url` 覆盖(原 `add` 第二次会失败)
/// - 不存在 → `git remote add`
/// 让"绑定仓库 → 改仓库地址"这种二次操作不会因 git 拒绝而抛错
#[tauri::command]
pub async fn git_init(
    app: AppHandle,
    path: String,
    branch: String,
    remote_url: Option<String>,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || {
        let branch = {
            let b = branch.trim();
            if b.is_empty() {
                "main"
            } else {
                b
            }
        };
        if let Err(e) = run_git(&path, &["init", "-b", branch]) {
            let msg = e.to_string();
            if msg.contains("unknown switch") || msg.contains("unrecognized option") {
                run_git(&path, &["init"])?;
                run_git(&path, &["checkout", "-b", branch])?;
            } else {
                return Err(e);
            }
        }
        if let Some(url) = remote_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            // 先查 origin 是否存在:已存在走 set-url,不存在走 add,保证幂等
            if run_git(&path, &["remote", "get-url", "origin"]).is_ok() {
                run_git(&path, &["remote", "set-url", "origin", url])?;
            } else {
                run_git(&path, &["remote", "add", "origin", url])?;
            }
        }
        let st = status(&path)?;
        cache_status(&path, &st);
        Ok(st)
    })
    .await?;
    publish_write_status(&app, &event_path, &status, "init", true);
    Ok(status)
}

/// 切换分支;create 为 true 时创建并切换(`git checkout -b`),
/// start_point 非空时以其为基点创建(可为本地分支或 origin/xxx 形式的远程分支)。
/// remote 为 true 时 branch 形如 "origin/feature":本地已有同名分支则直接切换,
/// 否则创建跟踪分支(`git checkout -b feature --track origin/feature`)
#[tauri::command]
pub async fn git_checkout(
    app: AppHandle,
    path: String,
    branch: String,
    create: bool,
    remote: bool,
    start_point: Option<String>,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || {
        checkout_blocking(&path, &branch, create, remote, start_point.as_deref())
    })
    .await?;
    publish_write_status(&app, &event_path, &status, "checkout", true);
    Ok(status)
}

pub(super) fn checkout_blocking(
    path: &str,
    branch: &str,
    create: bool,
    remote: bool,
    start_point: Option<&str>,
) -> AppResult<GitStatus> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    if create {
        match start_point.map(str::trim).filter(|s| !s.is_empty()) {
            Some(base) => run_git(path, &["checkout", "-b", branch, base])?,
            None => run_git(path, &["checkout", "-b", branch])?,
        };
    } else if remote {
        let short = branch.split_once('/').map(|(_, s)| s).unwrap_or(branch);
        if local_branch_names(path)?.iter().any(|b| b == short) {
            run_git(path, &["checkout", short])?;
        } else {
            run_git(path, &["checkout", "-b", short, "--track", branch])?;
        }
    } else {
        run_git(path, &["checkout", branch])?;
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

/// 提交更改,返回最新状态。
/// 判断未跟踪条目是否为嵌套 git 仓库:
/// git 对含 .git 的未跟踪目录只列目录本身(以 / 结尾);.git 可能是目录或 worktree gitfile
pub(super) fn is_nested_repo(path: &str, entry: &str) -> bool {
    entry.ends_with('/')
        && Path::new(path)
            .join(entry.trim_end_matches('/'))
            .join(".git")
            .exists()
}

/// 在当前调用内缓存已确认的嵌套仓库目录。
/// 仅缓存命中项,避免重复检查同一目录的 `.git` 路径。
pub(super) fn is_nested_repo_cached(
    path: &str,
    entry: &str,
    cache: &mut std::collections::HashSet<String>,
) -> bool {
    if cache.contains(entry) {
        return true;
    }
    let result = is_nested_repo(path, entry);
    if result {
        cache.insert(entry.to_string());
    }
    result
}

/// 列出未跟踪目录中的嵌套 git 仓库(返回不带结尾 / 的相对路径)。
/// 嵌套仓库是独立项目,不算本仓库的未提交内容
pub(super) fn nested_repo_dirs(path: &str) -> Vec<String> {
    let Ok(out) = run_git(path, &["ls-files", "--others", "--exclude-standard"]) else {
        return Vec::new();
    };
    // 缓存本次扫描中已确认的嵌套仓库
    let mut cache: std::collections::HashSet<String> = std::collections::HashSet::new();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| is_nested_repo_cached(path, l, &mut cache))
        .map(|l| l.trim_end_matches('/').to_string())
        .collect()
}

/// 参考 IDEA 提交模型:已暂存内容与未暂存修改(含已解决的冲突文件)始终提交;
/// 仅未跟踪文件需要显式勾选(include_untracked)才纳入;
/// 嵌套 git 仓库始终排除,避免被误加成 embedded gitlink(只存 commit 指针)。
/// paths 非空时走部分提交:仅暂存并提交指定路径(用 `commit --only`),
/// 不在 paths 中的已暂存文件不进入本次提交(分批/挑选提交场景)
#[tauri::command]
pub async fn git_commit(
    app: AppHandle,
    path: String,
    message: String,
    include_untracked: bool,
    paths: Option<Vec<String>>,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status =
        run_blocking(move || commit_blocking(&path, &message, include_untracked, paths)).await?;
    publish_write_status(&app, &event_path, &status, "commit", true);
    Ok(status)
}

pub(super) fn commit_blocking(
    path: &str,
    message: &str,
    include_untracked: bool,
    paths: Option<Vec<String>>,
) -> AppResult<GitStatus> {
    let message = message.trim();
    if message.is_empty() {
        return Err(AppError::coded(ErrorCode::GitCommitMessageRequired, ""));
    }
    if let Some(paths) = paths {
        // 部分提交(提交对话框勾选了文件子集):只暂存并提交这些路径,
        // 重命名的新旧路径由前端一并传入;commit --only 取这些路径的工作区内容,
        // 此前已暂存但未选中的文件不进入本次提交(合并进行中 git 会拒绝,错误透传)
        if paths.is_empty() {
            return Err(AppError::coded(ErrorCode::GitPathsRequired, ""));
        }
        let mut add_args: Vec<&str> = vec!["add", "-A", "--"];
        add_args.extend(paths.iter().map(String::as_str));
        run_git(path, &add_args)?;
        let mut commit_args: Vec<&str> = vec!["commit", "-m", message, "--only", "--"];
        commit_args.extend(paths.iter().map(String::as_str));
        run_git(path, &commit_args)?;
    } else if include_untracked {
        let nested = nested_repo_dirs(path);
        if nested.is_empty() {
            run_git(path, &["add", "-A"])?;
        } else {
            let mut args: Vec<String> = vec!["add".into(), "-A".into(), "--".into(), ".".into()];
            for dir in &nested {
                args.push(format!(":(exclude){dir}"));
            }
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            run_git(path, &arg_refs)?;
        }
        run_git(path, &["commit", "-m", message])?;
    } else {
        run_git(path, &["add", "-u"])?;
        run_git(path, &["commit", "-m", message])?;
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

/// 拉取远端。产生合并冲突时不算失败:返回冲突文件列表,由前端引导用户解决。
/// branch 指定拉取目标分支:为当前检出分支(或缺省)时走 `git pull`;
/// 为其他本地分支时不切换工作区,经 `git fetch <remote> <src>:<branch>` 快进更新引用
/// (分叉或分支被其他 worktree 占用时由 git 报错透传)
#[tauri::command]
pub async fn git_pull(
    app: AppHandle,
    path: String,
    branch: Option<String>,
) -> AppResult<GitPullResult> {
    let event_path = path.clone();
    let result = run_blocking(move || match branch {
        Some(b) if !b.is_empty() && current_branch(&path).as_deref() != Some(b.as_str()) => {
            pull_branch_blocking(&path, &b)
        }
        _ => pull_blocking(&path),
    })
    .await?;
    publish_write_status(&app, &event_path, &result.status, "pull", true);
    Ok(result)
}

pub(super) fn pull_blocking(path: &str) -> AppResult<GitPullResult> {
    let result = git_command(path).arg("pull").output()?;
    let conflicts = unmerged_files(path);
    if !result.status.success() && conflicts.is_empty() {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(if detail.is_empty() {
            AppError::coded(ErrorCode::GitPullFailed, "")
        } else {
            friendly_git_error(&detail)
        });
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(GitPullResult {
        status: st,
        conflicts,
    })
}

/// 拉取非当前检出的本地分支:不切换工作区,用 fetch refspec 快进更新本地引用。
/// 远端与源分支取该分支的 upstream;未配置 upstream 时回退到默认远端(优先 origin)
/// 的同名分支。非快进(分叉)或被其他 worktree 检出时 git 报错,经 friendly_git_error 透传
pub(super) fn pull_branch_blocking(path: &str, branch: &str) -> AppResult<GitPullResult> {
    let (remote, src) = match upstream_of(path, branch) {
        Some(pair) => pair,
        None => {
            let remote = default_push_remote(path)
                .ok_or_else(|| AppError::coded(ErrorCode::GitPullFailed, ""))?;
            (remote, branch.to_string())
        }
    };
    run_git(path, &["fetch", &remote, &format!("{src}:{branch}")])?;
    let st = status(path)?;
    cache_status(path, &st);
    Ok(GitPullResult {
        status: st,
        conflicts: Vec::new(),
    })
}

/// 当前检出分支名;detached HEAD 或命令失败时返回 None
pub(super) fn current_branch(path: &str) -> Option<String> {
    let out = git_command(path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty() && name != "HEAD").then_some(name)
}

/// 解析本地分支的 upstream,返回 (远端名, 远端分支名);未配置或已失效时返回 None
pub(super) fn upstream_of(path: &str, branch: &str) -> Option<(String, String)> {
    let out = git_command(path)
        .args([
            "rev-parse",
            "--abbrev-ref",
            &format!("{branch}@{{upstream}}"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    split_remote_branch(String::from_utf8_lossy(&out.stdout).trim())
}

/// 将 "origin/feature/x" 拆为 ("origin", "feature/x");不含 '/' 时无法判定远端,返回 None
pub(super) fn split_remote_branch(name: &str) -> Option<(String, String)> {
    let (remote, branch) = name.split_once('/')?;
    if remote.is_empty() || branch.is_empty() {
        return None;
    }
    Some((remote.to_string(), branch.to_string()))
}

/// 首推回退时的目标远端:优先 origin,否则取列表第一个远端;
/// 一个都没有返回 None(此时 `git push` 会先报无推送目标,走不到这里)
pub(super) fn default_push_remote(path: &str) -> Option<String> {
    let out = git_command(path).arg("remote").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let names: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if names.iter().any(|n| n == "origin") {
        Some("origin".to_string())
    } else {
        names.into_iter().next()
    }
}

/// 推送分支;branch 缺省或为当前检出分支时推送 HEAD,无 upstream(如新建分支首推)
/// 自动回退 `git push -u <remote> HEAD`,remote 优先 origin、否则取第一个远端。
/// branch 为其他本地分支时推送该分支:有 upstream 推到 upstream 对应分支,
/// 无 upstream 回退 `git push -u <默认远端> <branch>` 并建立跟踪
#[tauri::command]
pub async fn git_push(
    app: AppHandle,
    path: String,
    branch: Option<String>,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || match branch {
        Some(b) if !b.is_empty() && current_branch(&path).as_deref() != Some(b.as_str()) => {
            push_branch_blocking(&path, &b)
        }
        _ => push_blocking(&path),
    })
    .await?;
    publish_write_status(&app, &event_path, &status, "push", false);
    Ok(status)
}

/// 推送非当前检出的本地分支(不影响工作区)
pub(super) fn push_branch_blocking(path: &str, branch: &str) -> AppResult<GitStatus> {
    match upstream_of(path, branch) {
        Some((remote, src)) => {
            run_git(path, &["push", &remote, &format!("{branch}:{src}")])?;
        }
        None => {
            let remote = default_push_remote(path)
                .ok_or_else(|| AppError::coded(ErrorCode::GitNoTracking, ""))?;
            run_git(path, &["push", "-u", &remote, branch])?;
        }
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

/// 删除本地分支。force=false 用 -d(仅已合并分支,未合并报 git_branch_not_merged);
/// force=true 用 -D 强删。当前检出或被其他 worktree 占用的分支由 git 拒绝,错误透传
#[tauri::command]
pub async fn git_branch_delete(
    app: AppHandle,
    path: String,
    branch: String,
    force: bool,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || branch_delete_blocking(&path, &branch, force)).await?;
    publish_write_status(&app, &event_path, &status, "branch_delete", false);
    Ok(status)
}

pub(super) fn branch_delete_blocking(
    path: &str,
    branch: &str,
    force: bool,
) -> AppResult<GitStatus> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    let flag = if force { "-D" } else { "-d" };
    run_git(path, &["branch", flag, branch])?;
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

/// 删除远程分支。branch 形如 "origin/feature/x",拆出远端名与短名后执行
/// `git push <remote> --delete <short>`;名称不含远端前缀时报 git_branch_name_required,
/// 远端不存在或分支不存在等由 git 报错,经 friendly_git_error 透传
#[tauri::command]
pub async fn git_remote_branch_delete(
    app: AppHandle,
    path: String,
    branch: String,
) -> AppResult<GitStatus> {
    let event_path = path.clone();
    let status = run_blocking(move || remote_branch_delete_blocking(&path, &branch)).await?;
    publish_write_status(&app, &event_path, &status, "remote_branch_delete", false);
    Ok(status)
}

pub(super) fn remote_branch_delete_blocking(path: &str, branch: &str) -> AppResult<GitStatus> {
    let (remote, short) = split_remote_branch(branch.trim())
        .ok_or_else(|| AppError::coded(ErrorCode::GitBranchNameRequired, ""))?;
    run_git(path, &["push", &remote, "--delete", &short])?;
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

pub(super) fn push_blocking(path: &str) -> AppResult<GitStatus> {
    match run_git(path, &["push"]) {
        Ok(_) => {}
        Err(e) => {
            let no_upstream = e.to_string().contains("no upstream branch")
                || e.to_string().contains("has no upstream branch");
            if !no_upstream {
                return Err(e);
            }
            let Some(remote) = default_push_remote(path) else {
                return Err(e);
            };
            run_git(path, &["push", "-u", &remote, "HEAD"])?;
        }
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}
