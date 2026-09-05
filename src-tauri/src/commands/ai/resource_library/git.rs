//! 资源库 git 层:初始化、快照提交、remote 配置(聚合)、同步状态、
//! 首次远端导入(备份保留)、分叉 resolve(remote/local)、加密历史重写 + 强推。
//!
//! 复用 `commands::git` 的 `git_command` / `run_git`(禁用终端凭据交互、
//! Windows 隐藏黑窗、错误码映射与项目 git 一致)。资源库自身是目录内
//! `.git` 仓库;mcp.json 加密后的密文直接进 git,同步无需口令。

use std::fs;
use std::process::Output;

use crate::commands::git::{git_command, run_git};
use crate::error::{AppError, ErrorCode};
#[cfg(test)]
use crate::time_util::now_ts;

use super::crypto::clear_key;
use super::errors::{codes, RlError, RlResult};
#[cfg(test)]
use super::models::ImportResult;
use super::models::{SyncOutcome, SyncStatus};
use super::store::{remove_dir_tolerating_readonly, Library};

const GIT_NAME: &str = "RepoMeow";
const GIT_EMAIL: &str = "repomeow@localhost";
const GITIGNORE: &str = "*.repomeow-tmp\n*.repomeow-bak\n";

fn dir_str(lib: &Library) -> String {
    lib.root().to_string_lossy().into_owned()
}

/// 命令失败统一映射(与 run_git 的 GitCommandFailed 语义一致)
fn git_failed(op: &str, out: &Output) -> RlError {
    let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
    RlError::App(AppError::coded(
        ErrorCode::GitCommandFailed,
        format!("git {op}: {detail}"),
    ))
}

fn capture(dir: &str, args: &[&str]) -> RlResult<Output> {
    let out = git_command(dir)
        .args(args)
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if !out.status.success() {
        return Err(git_failed(&args.join(" "), &out));
    }
    Ok(out)
}

fn stdout_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(super) fn is_repo(lib: &Library) -> bool {
    lib.root().join(".git").exists()
}

/// git init + 本地固化配置(不依赖用户全局 git 配置);不写 .gitignore
/// (首次 checkout 远端树时,未跟踪的 .gitignore 会被 git 以「覆盖未跟踪文件」拒绝)
fn init_base(lib: &Library) -> RlResult<()> {
    if is_repo(lib) {
        return Ok(());
    }
    let dir = dir_str(lib);
    run_git(&dir, &["init"])?;
    for (key, value) in [
        ("user.name", GIT_NAME),
        ("user.email", GIT_EMAIL),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        run_git(&dir, &["config", key, value])?;
    }
    Ok(())
}

/// CRUD 路径用的完整 init:git init + 本地配置 + 补写 .gitignore
pub(super) fn init(lib: &Library) -> RlResult<()> {
    init_base(lib)?;
    ensure_gitignore(lib)?;
    Ok(())
}

/// .gitignore 缺失时补写;库已有提交则顺手入账(无提交时留给首次 CRUD 快照)
fn ensure_gitignore(lib: &Library) -> RlResult<()> {
    let gitignore = lib.root().join(".gitignore");
    if gitignore.exists() {
        return Ok(());
    }
    fs::write(gitignore, GITIGNORE)?;
    commit(lib, "补充 .gitignore")?;
    Ok(())
}

/// 工作区是否有未提交变更
pub(super) fn dirty(lib: &Library) -> RlResult<bool> {
    let dir = dir_str(lib);
    let out = git_command(&dir)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if !out.status.success() {
        return Err(git_failed("status --porcelain", &out));
    }
    Ok(!out.stdout.iter().all(|b| b.is_ascii_whitespace()))
}

/// 快照提交;无变更时幂等跳过,返回提交短 hash
pub(super) fn commit(lib: &Library, message: &str) -> RlResult<Option<String>> {
    if !dirty(lib)? {
        return Ok(None);
    }
    let dir = dir_str(lib);
    run_git(&dir, &["add", "-A"])?;
    run_git(
        &dir,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--no-verify",
            "-m",
            message,
        ],
    )?;
    let out = capture(&dir, &["rev-parse", "--short", "HEAD"])?;
    Ok(Some(stdout_text(&out)))
}

/// CRUD 自动快照:未初始化则先 init,再提交(无变更跳过)
pub(super) fn auto_commit(lib: &Library, message: &str) -> RlResult<Option<String>> {
    init(lib)?;
    commit(lib, message)
}

// ── remote 配置 ────────────────────────────────────────────────────────

pub(super) fn remote_get(lib: &Library) -> RlResult<Option<String>> {
    let dir = dir_str(lib);
    let out = git_command(&dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if out.status.success() {
        return Ok(Some(stdout_text(&out)));
    }
    // git 无 origin 时退出码 2:"error: No such remote 'origin'";
    // 非 git 仓库退出码 128 —— 二者都视为「无 remote」
    if out.status.code() == Some(2)
        || String::from_utf8_lossy(&out.stderr).contains("not a git repository")
    {
        return Ok(None);
    }
    Err(git_failed("remote get-url", &out))
}

pub(super) fn remote_set(lib: &Library, url: &str) -> RlResult<()> {
    // 用 init_base:remote 配置不应顺带产生 .gitignore 提交
    init_base(lib)?;
    let dir = dir_str(lib);
    if remote_get(lib)?.is_some() {
        run_git(&dir, &["remote", "set-url", "origin", url])?;
    } else {
        run_git(&dir, &["remote", "add", "origin", url])?;
    }
    Ok(())
}

pub(super) fn remote_remove(lib: &Library) -> RlResult<()> {
    if remote_get(lib)?.is_none() {
        return Ok(());
    }
    let dir = dir_str(lib);
    run_git(&dir, &["remote", "remove", "origin"])?;
    Ok(())
}

/// 远端同名分支的 OID(存在时);远端空仓库返回 None
fn remote_branch_oid(lib: &Library, name: &str) -> RlResult<Option<String>> {
    let dir = dir_str(lib);
    let out = git_command(&dir)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/remotes/origin/{name}"),
        ])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(stdout_text(&out)))
}

/// 本地仓库是否已有提交
fn has_commits(lib: &Library) -> RlResult<bool> {
    let dir = dir_str(lib);
    let out = git_command(&dir)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    Ok(out.status.success())
}

/// **聚合 remote 配置**(推荐入口,替代裸 `rl_remote_set`):
/// 1. 本地快照提交;2. 设置 URL;3. fetch;4. 决策:
///    - 本地无提交(全新库)+ 远端有内容 → **首次远端优先导入**(远端覆盖本地);
///    - 本地有提交 + 远端有内容 → 建立 upstream,纯快进则 pull/push,
///      分叉则原样透出 diverged,由 `rl_resolve_fork` 决策;
///    - 远端为空 → 推送本地(branch 参数可指定分支名,必要时改名本地分支)。
/// 网络失败不外抛,记录进 SyncOutcome。
pub(super) fn remote_configure_impl(
    lib: &Library,
    url: &str,
    requested_branch: Option<String>,
) -> RlResult<SyncOutcome> {
    let mut out = SyncOutcome::default();
    if url.trim().is_empty() {
        return Err(RlError::coded(ErrorCode::GitCloneUrlRequired.as_str(), ""));
    }
    fs::create_dir_all(lib.root())?;
    // init_base 不写 .gitignore:避免「首次远端优先导入」checkout 时冲突
    init_base(lib)?;
    // 快照本地现状(无变更跳过;全新空库不产生提交,供「远端优先导入」判定)
    commit(lib, "配置同步远端前快照")?;
    remote_set(lib, url.trim())?;
    match fetch(lib) {
        Ok(()) => out.fetched = true,
        Err(e) => return Ok(fail_out(out, e)),
    }
    let requested = requested_branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string);
    let remote_branch = match &requested {
        Some(b) => match remote_branch_oid(lib, b)? {
            Some(_) => Some(b.clone()),
            None => detect_remote_branch(lib)?,
        },
        None => detect_remote_branch(lib)?,
    };
    match (remote_branch, has_commits(lib)?) {
        (Some(rb), false) => {
            // 全新本地库:首次远端优先导入,远端内容覆盖空本地
            let dir = dir_str(lib);
            run_git(
                &dir,
                &["checkout", "-B", &rb, "--track", &format!("origin/{rb}")],
            )?;
            ensure_gitignore(lib)?;
            clear_key(lib.root());
            out.pulled = true;
        }
        (Some(rb), true) => {
            let dir = dir_str(lib);
            let _ = run_git(&dir, &["branch", "-u", &format!("origin/{rb}")]);
            let state = compute_state(lib)?;
            out.ahead = state.ahead;
            out.behind = state.behind;
            out.diverged = state.ahead > 0 && state.behind > 0;
            if !out.diverged {
                if state.behind > 0 {
                    pull_ff(lib)?;
                    out.pulled = true;
                }
                let after = compute_state(lib)?;
                out.ahead = after.ahead;
                out.behind = after.behind;
                if after.ahead > 0 {
                    push(lib, false)?;
                    out.pushed = true;
                }
            }
            ensure_gitignore(lib)?;
        }
        (None, _) => {
            // 远端为空:推送本地;branch 参数可指定本地分支名
            ensure_gitignore(lib)?;
            let dir = dir_str(lib);
            let local_branch = branch(lib)?
                .unwrap_or_else(|| requested.clone().unwrap_or_else(|| "main".to_string()));
            if let Some(want) = requested {
                if want != local_branch {
                    run_git(&dir, &["branch", "-M", &want])?;
                }
            }
            push(lib, false)?;
            out.pushed = true;
        }
    }
    out.ok = !out.diverged;
    Ok(out)
}

// ── 状态 ───────────────────────────────────────────────────────────────

pub(super) fn branch(lib: &Library) -> RlResult<Option<String>> {
    let dir = dir_str(lib);
    let out = git_command(&dir)
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if !out.status.success() {
        // 未创建提交(未出生 HEAD)或分离头
        return Ok(None);
    }
    Ok(Some(stdout_text(&out)))
}

/// 当前分支的上游(`origin/main` 形式);无上游返回 None
pub(super) fn upstream(lib: &Library) -> RlResult<Option<String>> {
    let dir = dir_str(lib);
    let out = git_command(&dir)
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(stdout_text(&out)))
}

pub(super) fn fetch(lib: &Library) -> RlResult<()> {
    run_git(&dir_str(lib), &["fetch", "origin"])?;
    Ok(())
}

/// 与上游的比较快照(供 sync/状态使用;需先 fetch)
#[derive(Debug, Default)]
pub(super) struct SyncState {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

pub(super) fn compute_state(lib: &Library) -> RlResult<SyncState> {
    let mut state = SyncState::default();
    state.branch = branch(lib)?;
    state.upstream = upstream(lib)?;
    state.dirty = dirty(lib)?;
    if state.upstream.is_some() {
        let dir = dir_str(lib);
        let out = capture(
            &dir,
            &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
        )?;
        let text = stdout_text(&out);
        let mut parts = text.split_whitespace();
        state.ahead = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        state.behind = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    }
    Ok(state)
}

/// 同步状态(配置了 remote 时先 fetch;网络错误按 git 错误码透传)
pub(super) fn sync_status_impl(lib: &Library) -> RlResult<SyncStatus> {
    let mut status = SyncStatus::default();
    if !is_repo(lib) {
        return Ok(status);
    }
    status.initialized = true;
    status.remote = remote_get(lib)?;
    status.branch = branch(lib)?;
    status.dirty = dirty(lib)?;
    status.last_sync = lib.read_state().last_sync;
    if status.remote.is_none() {
        return Ok(status);
    }
    let had_upstream = upstream(lib)?.is_some();
    match fetch(lib) {
        Ok(()) => {}
        // 远端分支已被删除:不算失败,以 remote_gone 标记透出
        Err(e) if e.code() == "git_remote_branch_gone" => {
            status.remote_gone = true;
            return Ok(status);
        }
        Err(e) => return Err(e),
    }
    // 通配 refspec fetch 在远端分支删除后可能清掉本地远端跟踪 ref(@{u} 失联);
    // 不同 git 版本行为不一,再以 ls-remote 直接向远端求证一次
    if had_upstream && upstream(lib)?.is_none() {
        status.remote_gone = true;
        return Ok(status);
    }
    if let Some(branch_name) = &status.branch {
        let dir = dir_str(lib);
        let out = git_command(&dir)
            .args(["ls-remote", "--heads", "origin", branch_name])
            .output()
            .map_err(|e| {
                RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))
            })?;
        if out.status.success() && stdout_text(&out).is_empty() {
            status.remote_gone = true;
            return Ok(status);
        }
    }
    let state = compute_state(lib)?;
    status.ahead = state.ahead;
    status.behind = state.behind;
    status.diverged = state.ahead > 0 && state.behind > 0;
    Ok(status)
}

// ── 推送 / 拉取 / 分叉解决 ─────────────────────────────────────────────

fn branch_required(lib: &Library) -> RlResult<String> {
    branch(lib)?.ok_or_else(|| RlError::coded(codes::NOT_INITIALIZED, ""))
}

/// 推送。`force_with_lease` 时**必须**带显式租约:以刚 fetch 到的
/// 远端跟踪 OID 为 expected 值;远端无同名分支(无租约可立)时走普通
/// `-u` 推送。**绝不退化为裸 `--force`**。
pub(super) fn push(lib: &Library, force_with_lease: bool) -> RlResult<()> {
    let dir = dir_str(lib);
    let branch_name = branch_required(lib)?;
    if remote_get(lib)?.is_none() {
        return Err(RlError::coded(codes::REMOTE_REQUIRED, ""));
    }
    if force_with_lease {
        match remote_branch_oid(lib, &branch_name)? {
            Some(oid) => {
                // 显式租约:<refname> 取被推的分支短名,expected 取刚 fetch 的 OID
                // (实测 refs/remotes/... 形式不生效,会退回非快进拒绝)
                let lease = format!("{branch_name}:{oid}");
                run_git(
                    &dir,
                    &[
                        "push",
                        &format!("--force-with-lease={lease}"),
                        "origin",
                        &branch_name,
                    ],
                )?;
            }
            None => {
                // 远端无该分支:无既有内容可覆盖,普通推送即可
                run_git(&dir, &["push", "-u", "origin", &branch_name])?;
            }
        }
    } else if upstream(lib)?.is_some() {
        run_git(&dir, &["push", "origin"])?;
    } else {
        run_git(&dir, &["push", "-u", "origin", &branch_name])?;
    }
    Ok(())
}

/// ff-only 合并上游(调用方需已 fetch 并预检)
pub(super) fn pull_ff(lib: &Library) -> RlResult<()> {
    let upstream_name = upstream(lib)?.ok_or_else(|| RlError::coded(codes::REMOTE_REQUIRED, ""))?;
    run_git(&dir_str(lib), &["merge", "--ff-only", &upstream_name])?;
    Ok(())
}

/// 显式推送:先 fetch;dirty / 落后 / 分叉给明确错误码
#[cfg(test)]
pub(super) fn push_now(lib: &Library) -> RlResult<()> {
    if remote_get(lib)?.is_none() {
        return Err(RlError::coded(codes::REMOTE_REQUIRED, ""));
    }
    fetch(lib)?;
    let state = compute_state(lib)?;
    if state.dirty {
        return Err(RlError::coded(codes::DIRTY, ""));
    }
    if state.ahead > 0 && state.behind > 0 {
        return Err(RlError::coded(codes::DIVERGED, ""));
    }
    if state.behind > 0 {
        return Err(RlError::coded(codes::BEHIND, ""));
    }
    push(lib, false)
}

/// 分叉解决:`remote` = 本地重置为远端;`local` = force-with-lease 强推本地。
/// 仅分叉态(ahead>0 且 behind>0)允许,方向由调用方固定传 "remote"/"local"。
pub(super) fn resolve_fork(lib: &Library, direction: &str) -> RlResult<()> {
    let direction = direction.trim();
    if direction != "remote" && direction != "local" {
        return Err(RlError::coded(
            codes::DIRECTION_INVALID,
            direction.to_string(),
        ));
    }
    if remote_get(lib)?.is_none() {
        return Err(RlError::coded(codes::REMOTE_REQUIRED, ""));
    }
    fetch(lib)?;
    let state = compute_state(lib)?;
    if !(state.ahead > 0 && state.behind > 0) {
        return Err(RlError::coded(codes::NOT_DIVERGED, ""));
    }
    if direction == "remote" {
        let upstream_name =
            upstream(lib)?.ok_or_else(|| RlError::coded(codes::REMOTE_REQUIRED, ""))?;
        run_git(&dir_str(lib), &["reset", "--hard", &upstream_name])?;
    } else {
        push(lib, true)?;
    }
    Ok(())
}

/// 一次自动/显式同步尝试:fetch → ff 合并 → 推送。任何网络/状态失败
/// 不外抛,记录进 SyncOutcome(本地保存永不因网络失败整体报错)。
pub(super) fn sync_once_impl(lib: &Library) -> SyncOutcome {
    let mut out = SyncOutcome::default();
    if !is_repo(lib) {
        return fail_out(out, RlError::coded(codes::NOT_INITIALIZED, ""));
    }
    match remote_get(lib) {
        Ok(Some(_)) => {}
        Ok(None) => {
            out.ok = true;
            return out;
        }
        Err(e) => return fail_out(out, e),
    }
    match fetch(lib) {
        Ok(()) => out.fetched = true,
        Err(e) => return fail_out(out, e),
    }
    let state = match compute_state(lib) {
        Ok(s) => s,
        Err(e) => return fail_out(out, e),
    };
    out.ahead = state.ahead;
    out.behind = state.behind;
    out.diverged = state.ahead > 0 && state.behind > 0;
    if state.dirty {
        return fail_out(out, RlError::coded(codes::DIRTY, ""));
    }
    if out.diverged {
        return fail_out(out, RlError::coded(codes::DIVERGED, ""));
    }
    if state.behind > 0 {
        if let Err(e) = pull_ff(lib) {
            return fail_out(out, e);
        }
        out.pulled = true;
    }
    let after = match compute_state(lib) {
        Ok(s) => s,
        Err(e) => return fail_out(out, e),
    };
    out.ahead = after.ahead;
    out.behind = after.behind;
    if after.ahead > 0 {
        match push(lib, false) {
            Ok(()) => out.pushed = true,
            Err(e) => return fail_out(out, e),
        }
    }
    out.ok = true;
    out
}

fn fail_out(mut out: SyncOutcome, e: RlError) -> SyncOutcome {
    out.ok = false;
    out.error_code = Some(e.code().to_string());
    out.error_message = Some(e.message());
    out
}

// ── 首次远端导入(本地备份保留)─────────────────────────────────────────

fn detect_remote_branch(lib: &Library) -> RlResult<Option<String>> {
    let dir = dir_str(lib);
    // fetch 时 git 会按远端 HEAD 建立 refs/remotes/origin/HEAD,以它为准最稳
    let out = git_command(&dir)
        .args(["symbolic-ref", "--short", "-q", "refs/remotes/origin/HEAD"])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if out.status.success() {
        if let Some(name) = stdout_text(&out).strip_prefix("origin/") {
            return Ok(Some(name.to_string()));
        }
    }
    for name in ["main", "master"] {
        let tracking = format!("origin/{name}");
        let out = git_command(&dir)
            .args(["rev-parse", "--verify", "--quiet", &tracking])
            .output()
            .map_err(|e| {
                RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))
            })?;
        if out.status.success() {
            return Ok(Some(name.to_string()));
        }
    }
    // 兜底:远端 HEAD 悬空(如 bare 库 HEAD 仍指向不存在的 master)时,
    // 取任一实际远端分支(如推上来的 dev)
    let out = git_command(&dir)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin/",
        ])
        .output()
        .map_err(|e| RlError::App(AppError::coded(ErrorCode::GitCommandFailed, e.to_string())))?;
    if out.status.success() {
        for line in stdout_text(&out).lines() {
            if let Some(name) = line.trim().strip_prefix("origin/") {
                return Ok(Some(name.to_string()));
            }
        }
    }
    // 远端为空仓库
    Ok(None)
}

/// 首次从远端导入:
/// - `force=false`:本地已有 git 仓库或已有文件 → `import_conflict`
/// - `force=true`:先把本地原目录整目录改名备份为
///   `<库目录名>.backup-<ts>`(仓库外,内容完整保留),再导入远端;
///   导入成功保留备份目录并在结果中告知路径,失败恢复原目录。
#[cfg(test)]
pub(super) fn import_remote(lib: &Library, url: &str, force: bool) -> RlResult<ImportResult> {
    if url.trim().is_empty() {
        return Err(RlError::coded(ErrorCode::GitCloneUrlRequired.as_str(), ""));
    }
    // force=true 允许覆盖(先备份);仅非 force 时对已有 git 仓库/内容报冲突
    if !force && (is_repo(lib) || lib.has_content()) {
        return Err(RlError::coded(codes::IMPORT_CONFLICT, dir_str(lib)));
    }
    let backup: Option<std::path::PathBuf> = if force && lib.root().exists() && lib.has_content() {
        let backup = lib.backup_dir(now_ts());
        fs::rename(lib.root(), &backup)?;
        Some(backup)
    } else {
        None
    };
    let run = || -> RlResult<()> {
        fs::create_dir_all(lib.root())?;
        // init_base 不写 .gitignore,避免 checkout 远端树时被「覆盖未跟踪文件」拒绝
        init_base(lib)?;
        let dir = dir_str(lib);
        run_git(&dir, &["remote", "add", "origin", url.trim()])?;
        fetch(lib)?;
        if let Some(name) = detect_remote_branch(lib)? {
            let tracking = format!("origin/{name}");
            run_git(&dir, &["checkout", "-b", &name, "--track", &tracking])?;
        }
        ensure_gitignore(lib)?;
        Ok(())
    };
    match run() {
        Ok(()) => {
            // 远端内容已替换本地:作废本进程内旧密钥
            clear_key(lib.root());
            Ok(ImportResult {
                backup: backup.map(|b| b.to_string_lossy().into_owned()),
            })
        }
        Err(e) => {
            if let Some(backup) = &backup {
                // 清理半成品并恢复原目录
                let _ = remove_dir_tolerating_readonly(lib.root());
                let _ = fs::rename(backup, lib.root());
            }
            Err(e)
        }
    }
}

// ── 加密历史重写 + 强推(enable/disable 共用)───────────────────────────

/// 重建 git 历史:清除含明文 MCP 的旧提交。先记下 remote URL
/// (重建会抹掉 .git/config),全新 init + 单次提交后重挂 remote,
/// fetch 建立租约,再 `--force-with-lease`(显式 expected OID)强推;
/// 网络失败不阻断本地重建。
pub(super) fn rewrite_and_push(lib: &Library, message: &str) -> SyncOutcome {
    let mut out = SyncOutcome::default();
    let url = match remote_get(lib) {
        Ok(u) => u,
        Err(e) => return fail_out(out, e),
    };
    if let Err(e) = rewrite_history(lib, message) {
        return fail_out(out, e);
    }
    let Some(url) = url else {
        out.ok = true;
        return out;
    };
    if let Err(e) = remote_set(lib, &url).and_then(|_| fetch(lib)) {
        return fail_out(out, e);
    }
    out.fetched = true;
    match push(lib, true) {
        Ok(()) => out.pushed = true,
        Err(e) => return fail_out(out, e),
    }
    out.ok = true;
    out
}

/// 保留分支名,全新 init + 单次快照提交,旧历史不可达
fn rewrite_history(lib: &Library, message: &str) -> RlResult<()> {
    let keep_branch = branch(lib)?.unwrap_or_else(|| "main".to_string());
    let git_dir = lib.root().join(".git");
    if git_dir.exists() {
        remove_dir_tolerating_readonly(&git_dir)?;
    }
    init(lib)?;
    let dir = dir_str(lib);
    run_git(&dir, &["checkout", "-B", &keep_branch])?;
    commit(lib, message)?;
    Ok(())
}
