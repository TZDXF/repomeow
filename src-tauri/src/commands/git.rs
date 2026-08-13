use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Window, ipc::Channel};
use tokio::sync::Semaphore;

use crate::commands::account;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{
    GitBranchTrack, GitBranches, GitCommitContext, GitCommitFile, GitCommitFileDiff, GitCommitInfo,
    GitGraphBatch, GitGraphCommit, GitMergeResult, GitPullResult, GitRebaseResult, GitStatus,
    GitUntrackedFile, GitUser, GitWorktree,
};

/// 后台 fetch 并发上限(超出排队)
static FETCH_PERMITS: OnceLock<Semaphore> = OnceLock::new();
/// 单次 fetch 的总超时(覆盖 ssh:// 等非 http 协议;http 协议另有低速/连接超时配置)
const FETCH_TIMEOUT: Duration = Duration::from_secs(90);
/// fetch 失败后的基础退避间隔,随连续失败次数指数增长,封顶 15 分钟
const FETCH_RETRY_BASE: Duration = Duration::from_secs(30);
const FETCH_RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// fetch 治理状态:进行中去重 + 失败退避
struct FetchTracker {
    /// 正在 fetch 的路径(进行中不重复发起)
    in_progress: HashSet<String>,
    /// 路径 → (下次允许 fetch 的时刻, 连续失败次数)
    retry_after: HashMap<String, (Instant, u32)>,
}

static FETCH_TRACKER: OnceLock<Mutex<FetchTracker>> = OnceLock::new();

fn fetch_tracker() -> &'static Mutex<FetchTracker> {
    FETCH_TRACKER.get_or_init(|| {
        Mutex::new(FetchTracker {
            in_progress: HashSet::new(),
            retry_after: HashMap::new(),
        })
    })
}

/// 该路径当前是否允许发起 fetch(不在进行中、不在退避期)
fn fetch_due(path: &str) -> bool {
    let tracker = fetch_tracker().lock().unwrap();
    if tracker.in_progress.contains(path) {
        return false;
    }
    match tracker.retry_after.get(path) {
        Some((at, _)) => *at <= Instant::now(),
        None => true,
    }
}

/// fetch 结束回调:成功清除退避记录;失败按连续失败次数指数退避,
/// 弱网/断网时后台循环不会每 30s 重复撞网络
fn fetch_finished(path: &str, ok: bool) {
    let mut tracker = fetch_tracker().lock().unwrap();
    tracker.in_progress.remove(path);
    if ok {
        tracker.retry_after.remove(path);
    } else {
        let fails = tracker
            .retry_after
            .get(path)
            .map(|(_, f)| *f)
            .unwrap_or(0)
            + 1;
        let backoff = FETCH_RETRY_BASE
            .saturating_mul(2u32.saturating_pow(fails.saturating_sub(1)))
            .min(FETCH_RETRY_MAX);
        tracker
            .retry_after
            .insert(path.to_string(), (Instant::now() + backoff, fails));
    }
}

/// 进行中的克隆任务(job_id -> 子进程),供 cancel_git_clone 查找并 kill
static CLONE_JOBS: OnceLock<tokio::sync::Mutex<HashMap<String, tokio::process::Child>>> =
    OnceLock::new();

fn clone_jobs() -> &'static tokio::sync::Mutex<HashMap<String, tokio::process::Child>> {
    CLONE_JOBS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// 所有运行中的 git 子进程 PID(fetch + clone 统一登记),
/// 供应用退出钩子 cleanup_on_exit 在拿不到句柄时按 PID 杀进程树
static GIT_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

fn git_pids() -> &'static Mutex<HashSet<u32>> {
    GIT_PIDS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// RAII 守卫:构造时登记 PID,drop 时注销。保证无论函数从成功/失败/超时/
/// 取消哪条路径返回都自动移除,不会残留已退出的 PID(或残留也无害——
/// cleanup_on_exit 对不存在的 PID taskkill 只会返回错误,不影响其他 PID)
struct TrackedPid(Option<u32>);

impl TrackedPid {
    fn new(pid: Option<u32>) -> Self {
        if let Some(p) = pid {
            git_pids().lock().unwrap().insert(p);
        }
        Self(pid)
    }
}

impl Drop for TrackedPid {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            git_pids().lock().unwrap().remove(&p);
        }
    }
}

/// 应用退出收尾:杀掉所有仍在运行的 git 子进程(fetch/clone 及其
/// remote-helper 孙进程整棵进程树)。由 lib.rs 的 RunEvent::Exit 钩子调用。
///
/// 走 PID 层而非句柄层:fetch child 句柄困在 spawn 出的 task 内部,
/// clone child 在 async Mutex(CLONE_JOBS)里,退出钩子很难可靠触及;
/// 而 PID 是 spawn 后立即拷出的独立副本,始终可达。
pub fn cleanup_on_exit() {
    // 1) PID 层:taskkill 杀整棵进程树(覆盖孙进程)
    let pids: Vec<u32> = git_pids().lock().unwrap().drain().collect();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for pid in pids {
            let mut cmd = Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let _ = cmd.output();
        }
    }
    // 2) 兜底:回收 clone 句柄(CLONE_JOBS 是 async Mutex,try_lock 重试几次)
    for _ in 0..20 {
        if let Ok(mut jobs) = clone_jobs().try_lock() {
            for (_, mut child) in jobs.drain() {
                let _ = child.start_kill();
                let _ = child.wait();
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GitUpdatedPayload {
    pub project_id: i64,
    pub remote_ahead: i32,
    pub last_fetch_at: i64,
}

/// 构造 git 命令:禁用终端凭据交互(GUI 应用无人应答会挂起,凭据管理器
/// helper 弹窗不受影响),Windows 下隐藏控制台黑窗
fn git_command_raw() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

pub(crate) fn git_command(path: &str) -> Command {
    let mut cmd = git_command_raw();
    cmd.arg("-C").arg(path);
    cmd
}

/// 执行 git 命令,非零退出时取 stderr(兜底 stdout)转为友好错误
fn run_git(path: &str, args: &[&str]) -> AppResult<Output> {
    let output = git_command(path).args(args).output()?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    Err(if detail.is_empty() {
        AppError::coded(
            ErrorCode::GitCommandFailed,
            format!("args={} status={}", args.join(" "), output.status),
        )
    } else {
        friendly_git_error(&detail)
    })
}

/// 将 git 原始 stderr 转为简洁友好的错误:
/// 1. 过滤环境噪音行(如 OpenSSH 后量子密钥交换警告)
/// 2. 常见错误模式映射为带错误码的 Coded 错误(前端按 code 走 i18n,
///    此处 message 仅保留技术上下文);未识别时返回清理后的原文(External→Coded)
///
/// 注意:`push_blocking` 依赖原文匹配 "no upstream branch",映射规则不得覆盖该短语
fn friendly_git_error(raw: &str) -> AppError {
    use crate::error::ErrorCode;

    // 噪音行:SSH/网络层打印的警告,与 git 操作结果无关
    const NOISE: &[&str] = &[
        "post-quantum",
        "store now, decrypt later",
        "openssh.com/pq.html",
        "The server may need to be upgraded",
    ];
    let cleaned: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !l.starts_with("** "))
        .filter(|l| !NOISE.iter().any(|n| l.contains(n)))
        .collect();
    let text = cleaned.join("\n");
    if text.is_empty() {
        return AppError::coded(ErrorCode::GitNoiseFallback, "");
    }
    let coded = |code: ErrorCode, message: &str| AppError::Coded {
        code,
        message: message.into(),
    };

    // 本地修改/未跟踪文件会被合并或切换分支覆盖
    if text.contains("Your local changes to the following files would be overwritten by") {
        return coded(ErrorCode::GitLocalChangesConflict, "");
    }
    if text.contains("The following untracked working tree files would be overwritten by") {
        return coded(ErrorCode::GitUntrackedConflict, "");
    }

    // 认证与权限
    if text.contains("Permission denied (publickey") {
        return coded(ErrorCode::GitSshAuthFailed, "");
    }
    if text.contains("Host key verification failed") {
        return coded(ErrorCode::GitHostKeyFailed, "");
    }
    if text.contains("Authentication failed") || text.contains("Invalid username or password") {
        return coded(ErrorCode::GitAuthFailed, "");
    }
    if text.contains("Repository not found") || text.contains("repository not found") {
        return coded(ErrorCode::GitRepoNotFound, "");
    }

    // 网络
    if text.contains("Could not resolve host")
        || text.contains("Temporary failure in name resolution")
    {
        return coded(ErrorCode::GitNetworkDns, "");
    }
    if text.contains("Connection timed out")
        || text.contains("Connection refused")
        || text.contains("Connection reset")
        || text.contains("Failed to connect to")
    {
        return coded(ErrorCode::GitNetworkConnect, "");
    }

    // 推送/拉取策略
    if text.contains("failed to push some refs") {
        if text.contains("non-fast-forward")
            || text.contains("fetch first")
            || text.contains("Updates were rejected")
        {
            return coded(ErrorCode::GitPushRejected, "");
        }
        return AppError::coded(ErrorCode::GitPushFailed, text);
    }
    if text.contains("You have divergent branches")
        || text.contains("Need to specify how to reconcile divergent branches")
    {
        return coded(ErrorCode::GitDiverged, "");
    }
    // 上游远程分支已被删除:pull 当前分支时报 "no such ref was fetched",
    // fetch/pull 指定分支时报 "couldn't find remote ref"(不同 git 版本大小写不一)
    if text.contains("no such ref was fetched")
        || text.to_ascii_lowercase().contains("couldn't find remote ref")
    {
        return coded(ErrorCode::GitRemoteBranchGone, "");
    }
    if text.contains("There is no tracking information") {
        return coded(ErrorCode::GitNoTracking, "");
    }
    if text.contains("not a git repository") {
        return coded(ErrorCode::NotGitRepository, "");
    }

    // worktree / 分支占用
    if text.contains("is already checked out at") {
        return coded(ErrorCode::GitBranchCheckedOut, "");
    }
    if text.contains("contains modified or untracked files") {
        return coded(ErrorCode::GitWorktreeDirty, "");
    }
    if text.contains("branch named") && text.contains("already exists") {
        return coded(ErrorCode::GitBranchExists, "");
    }
    // 删除分支:未完全合并(git 建议 -D 强删),前端据此引导强制删除
    if text.contains("is not fully merged") {
        return coded(ErrorCode::GitBranchNotMerged, "");
    }

    // 未识别:整段清理后原文作为 message 携带
    AppError::coded(ErrorCode::GitCommandFailed, text)
}

/// 阻塞任务放入 tokio 线程池执行。
/// 同步 #[tauri::command] 在主线程跑,git 子进程(尤其 push/pull 网络操作)会卡死 UI
async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::coded(ErrorCode::GitTaskFailed, e.to_string()))?
}

pub fn status(path: &str) -> AppResult<GitStatus> {
    let output = git_command(path)
        .args(["status", "--porcelain=v2", "--branch"])
        .output()?;
    if !output.status.success() {
        // 不是 git 仓库(git 退出码 128)
        return Ok(GitStatus::default());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut st = parse_porcelain(path, &text);
    st.last_commit_at = last_commit_at(path);
    Ok(st)
}

/// HEAD 最新提交时间(Unix 秒)。无提交(空仓库)或命令失败时返回 None
fn last_commit_at(path: &str) -> Option<i64> {
    let output = git_command(path)
        .args(["log", "-1", "--format=%ct"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

// ── 本地状态缓存 ─────────────────────────────────────────────────────────

/// 本地 git 状态缓存 TTL:详情页/批量刷新等高频调用直接命中,
/// 后台刷新循环每 30s 全量重查一次,15s 内命中即视为足够新
const STATUS_TTL: Duration = Duration::from_secs(15);

struct CachedStatus {
    status: GitStatus,
    at: Instant,
}

/// 路径 → 最近一次本地状态(写操作后主动回填,保持一致性)
static STATUS_CACHE: OnceLock<Mutex<HashMap<String, CachedStatus>>> = OnceLock::new();

fn status_cache() -> &'static Mutex<HashMap<String, CachedStatus>> {
    STATUS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 缓存化状态查询:命中且未过期直接返回,否则执行 status() 并回填。
/// force 为 true 时绕过缓存强制重查(用户主动刷新/git 写操作后)
fn status_cached(path: &str, force: bool) -> AppResult<GitStatus> {
    if !force {
        if let Some(entry) = status_cache().lock().unwrap().get(path) {
            if entry.at.elapsed() < STATUS_TTL {
                return Ok(entry.status.clone());
            }
        }
    }
    let st = status(path)?;
    status_cache().lock().unwrap().insert(
        path.to_string(),
        CachedStatus {
            status: st.clone(),
            at: Instant::now(),
        },
    );
    Ok(st)
}

/// 写操作完成后回填缓存(返回的新状态即最新,不等待 TTL 过期)
fn cache_status(path: &str, st: &GitStatus) {
    status_cache().lock().unwrap().insert(
        path.to_string(),
        CachedStatus {
            status: st.clone(),
            at: Instant::now(),
        },
    );
}

/// 路径变更(重定向/移动/删除)后清除旧路径缓存
pub fn invalidate_status(path: &str) {
    status_cache().lock().unwrap().remove(path);
}

/// fetch 远端(无 remote 时跳过),返回最新状态。
/// http 协议带连接/低速超时配置:弱网/断网时 git fetch 默认无限期挂起
/// (TCP 重传可达数分钟),这里由 git 自身在慢速连接时主动中止。
/// 后台 fetch 走 fetch_with_timeout(带进程 kill 兜底),此同步版保留给测试
#[cfg_attr(not(test), allow(dead_code))]
pub fn fetch_and_status(path: &str) -> AppResult<GitStatus> {
    let remotes = git_command(path).arg("remote").output()?;
    if remotes.status.success() && !String::from_utf8_lossy(&remotes.stdout).trim().is_empty() {
        // 失败(如离线)不阻断,退回本地已知状态
        let _ = git_command(path)
            .args([
                "-c",
                "http.connectTimeout=10",
                "-c",
                "http.lowSpeedLimit=1000",
                "-c",
                "http.lowSpeedTime=30",
                "fetch",
                "--quiet",
            ])
            .output();
    }
    status(path)
}

/// 解析 `git status --porcelain=v2 --branch` 输出。
/// 嵌套 git 仓库(含 .git 的未跟踪目录)不计入未跟踪数:它是独立项目,不算本仓库的改动
fn parse_porcelain(path: &str, text: &str) -> GitStatus {
    let mut st = GitStatus {
        is_repo: true,
        ..Default::default()
    };
    for line in text.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            st.branch = Some(head.trim().to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            // 形如 "+2 -1"
            for part in ab.split_whitespace() {
                if let Some(a) = part.strip_prefix('+') {
                    st.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = part.strip_prefix('-') {
                    st.behind = b.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // 普通/重命名条目: 第 3-4 字节是 XY 状态码
            let bytes = line.as_bytes();
            if bytes.len() >= 4 {
                if bytes[2] != b'.' {
                    st.staged += 1;
                }
                if bytes[3] != b'.' {
                    st.modified += 1;
                }
            }
        } else if line.starts_with("u ") {
            st.modified += 1; // 冲突文件仍计入未暂存,保持「干净」判断语义
            st.conflicted += 1;
        } else if let Some(entry) = line.strip_prefix("? ") {
            // porcelain 对特殊字符路径做 C 风格引号转义,去掉外层引号再判断
            let entry = entry.trim().trim_matches('"');
            if !is_nested_repo(path, entry) {
                st.untracked += 1;
            }
        }
    }
    st.remote_ahead = st.behind;
    st
}

#[tauri::command]
pub async fn get_git_status(path: String, force: Option<bool>) -> AppResult<GitStatus> {
    let force = force.unwrap_or(false);
    run_blocking(move || status_cached(&path, force)).await
}

#[derive(Debug, Clone, Serialize)]
pub struct GitRemote {
    pub name: String,
    pub url: String,
}

// ── 批量查询与后台刷新循环 ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct GitStatusItem {
    pub path: String,
    pub status: GitStatus,
}

/// 批量状态查询的并发上限(git 子进程数量)
const STATUS_CONCURRENCY: usize = 8;

/// 批量查询多个路径的 git 状态(带缓存),结果按路径排序返回;
/// 单个路径查询失败时跳过,不阻断其他项目
pub async fn refresh_statuses_batch(paths: &[String], force: bool) -> Vec<GitStatusItem> {
    let semaphore = Arc::new(Semaphore::new(STATUS_CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    for path in paths {
        let path = path.clone();
        let semaphore = semaphore.clone();
        set.spawn(async move {
            let _permit = semaphore.acquire().await;
            // spawn_blocking 闭包独立持有 path 副本,避免与外层 async move 冲突
            let st = tokio::task::spawn_blocking({
                let path = path.clone();
                move || status_cached(&path, force)
            })
            .await;
            (path, st)
        });
    }
    let mut items = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((path, Ok(Ok(st)))) = joined {
            items.push(GitStatusItem { path, status: st });
        }
    }
    items.sort_by(|a, b| a.path.cmp(&b.path));
    items
}

/// 批量获取多个项目的 git 状态(带缓存):一次 IPC 返回全部,替代逐项目轮询。
/// force 为 true 时绕过缓存强制重查(启动/用户主动刷新)
#[tauri::command]
pub async fn refresh_all_git_status(
    paths: Vec<String>,
    force: bool,
) -> AppResult<Vec<GitStatusItem>> {
    Ok(refresh_statuses_batch(&paths, force).await)
}

/// 状态刷新周期(与原前端轮询间隔一致)
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// 启动后首轮刷新的延迟:先让首屏渲染完成,避免启动瞬间与列表请求抢资源
const STATUS_REFRESH_FIRST_DELAY: Duration = Duration::from_secs(3);

/// 读取所有未归档项目的 (id, path)
fn list_active_project_paths(app: &AppHandle) -> Vec<(i64, String)> {
    let Some(db) = app.try_state::<Db>() else {
        return Vec::new();
    };
    let conn = db.0.lock().unwrap();
    let mut stmt = match conn.prepare("SELECT id, path FROM projects WHERE archived_at IS NULL") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// 后台 git 状态刷新循环(替代前端 setInterval 轮询):
/// 启动 3s 后执行首轮(先让首屏渲染完成,避免启动瞬间与列表请求抢资源),
/// 之后每 30s 批量查询所有未归档项目状态(带缓存)并事件推送全量,
/// 随后调度一轮后台 fetch(进行中去重 + 失败退避 + 超时 kill)。
/// 网络失败只在这里发生一次,不再由前端逐项目轮询放大
pub async fn status_refresher_loop(app: AppHandle) {
    tokio::time::sleep(STATUS_REFRESH_FIRST_DELAY).await;
    loop {
        let projects = list_active_project_paths(&app);
        if !projects.is_empty() {
            let paths: Vec<String> = projects.iter().map(|(_, p)| p.clone()).collect();
            let items = refresh_statuses_batch(&paths, false).await;
            if !items.is_empty() {
                let _ = app.emit("git://status-updated", items);
            }
            for (id, path) in &projects {
                fetch_schedule(&app, *id, path.clone());
            }
        }
        tokio::time::sleep(STATUS_REFRESH_INTERVAL).await;
    }
}

/// 列出所有 remote 及其地址(非仓库或无 remote 返回空列表)
#[tauri::command]
pub async fn list_git_remotes(path: String) -> AppResult<Vec<GitRemote>> {
    run_blocking(move || list_remotes_blocking(&path)).await
}

fn list_remotes_blocking(path: &str) -> AppResult<Vec<GitRemote>> {
    // git remote -v 一次进程取全(替代 git remote + 逐 remote get-url 的 N+1 子进程)
    let output = git_command(path).args(["remote", "-v"]).output()?;
    if !output.status.success() {
        return Ok(vec![]);
    }
    Ok(parse_remote_verbose(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// 解析 `git remote -v` 输出:每行形如 "origin\t<url> (fetch)" / "origin\t<url> (push)",
/// 每个 remote 取首个(fetch)条目;无 URL 的 remote 不会出现在输出中
fn parse_remote_verbose(stdout: &str) -> Vec<GitRemote> {
    let mut out: Vec<GitRemote> = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url)) = (parts.next(), parts.next()) else {
            continue;
        };
        if url.is_empty() || out.iter().any(|r| r.name == name) {
            continue;
        }
        out.push(GitRemote {
            name: name.to_string(),
            url: url.to_string(),
        });
    }
    out
}

/// 带超时的后台 fetch:
/// - http(s) 协议由 git 连接/低速超时配置兜底(慢速连接 30s 无进展即中止)
/// - ssh 等其他协议由外层 timeout 兜底,超时 kill 进程树
/// 无 remote 的仓库直接视为成功(无需退避)
async fn fetch_with_timeout(path: &str) -> bool {
    let has_remote = git_command(path)
        .arg("remote")
        .output()
        .map(|o| o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false);
    if !has_remote {
        return true;
    }
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args([
            "-c",
            "http.connectTimeout=10",
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=30",
            "fetch",
            "--quiet",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // tokio::process::Command 在 Windows 上原生提供 creation_flags,无需 import
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    // 登记 PID 供应用退出钩子按 PID 清理(句柄在当前 task 内部,退出钩子够不到)
    let _tracked = TrackedPid::new(child.id());
    match tokio::time::timeout(FETCH_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(_)) => false,
        Err(_) => {
            // 超时:强制结束进程树(fetch 会派生 remote helper 孙进程)
            kill_process_tree(child);
            false
        }
    }
}

/// 强制结束 git 进程树(Windows 用 taskkill /T /F 覆盖孙进程)
fn kill_process_tree(mut child: tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let _ = cmd.output();
    }
    // 非 Windows 主路径;Windows 上作为 taskkill 的兜底(重复 kill 无害)
    let _ = child.start_kill();
    let _ = child.wait();
}

/// 调度一次后台 fetch(进行中/退避期跳过)。
/// fetch 成功后强制重查本地状态回填缓存,并把远端领先数经 "git://updated"
/// 广播给所有窗口(原实现只发触发窗口,广播后多窗口状态天然一致)
fn fetch_schedule(app: &AppHandle, project_id: i64, path: String) {
    if !fetch_due(&path) {
        return;
    }
    fetch_tracker()
        .lock()
        .unwrap()
        .in_progress
        .insert(path.clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let semaphore = FETCH_PERMITS.get_or_init(|| Semaphore::new(3));
        let _permit = semaphore.acquire().await;
        let ok = fetch_with_timeout(&path).await;
        if ok {
            let st = tokio::task::spawn_blocking({
                let path = path.clone();
                move || status_cached(&path, true)
            })
            .await;
            if let Ok(Ok(st)) = st {
                if st.is_repo {
                    let payload = GitUpdatedPayload {
                        project_id,
                        remote_ahead: st.behind,
                        last_fetch_at: chrono::Utc::now().timestamp(),
                    };
                    let _ = app.emit("git://updated", payload);
                }
            }
        }
        fetch_finished(&path, ok);
    });
}

/// 后台 fetch:不返回数据,完成后 emit "git://updated"
#[tauri::command]
pub fn fetch_git_remote_async(window: Window, project_id: i64, path: String) {
    fetch_schedule(window.app_handle(), project_id, path);
}

/// 当前处于合并冲突状态的文件(相对仓库根的路径)
fn unmerged_files(path: &str) -> Vec<String> {
    let Ok(out) = git_command(path)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn local_branch_names(path: &str) -> AppResult<Vec<String>> {
    // 不用 %(refname:short):存在与分支同名的 remote 时(如分支 zc + remote zc),
    // git 为消歧会输出 "heads/zc",而 git log %D 装饰只做 refs/heads/ 前缀剥离(仍显示 "zc"),
    // 两套命名不一致会导致图谱按分支名定位顶端提交失败;这里与 %D 保持一致
    let out = run_git(path, &["for-each-ref", "--format=%(refname)", "refs/heads"])?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().strip_prefix("refs/heads/").map(String::from))
        .collect())
}

/// 解析 `%(upstream:track)` 的输出:`[ahead 2, behind 3]` / `[ahead 1]` / `[behind 5]` / `[gone]` / 空。
/// for-each-ref 是 plumbing,该格式硬编码不受本地化影响;gone/空 均返回 (0, 0)
fn parse_upstream_track(track: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    for part in inner.split(',') {
        let mut segs = part.trim().split_whitespace();
        let Some(word) = segs.next() else { continue };
        let num: u32 = segs.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        match word {
            "ahead" => ahead = num,
            "behind" => behind = num,
            _ => {}
        }
    }
    (ahead, behind)
}

/// 本地分支名 + upstream 跟踪差值,一次 for-each-ref 取全。
/// 与 local_branch_names 同样不用 %(refname:short)(同名 remote 消歧问题);
/// upstream 取完整 refname 自行剥前缀,避免 %(upstream:short) 的同类歧义
fn local_branches_with_tracking(path: &str) -> AppResult<(Vec<String>, Vec<GitBranchTrack>)> {
    let out = run_git(
        path,
        &[
            "for-each-ref",
            "--format=%(refname)%09%(upstream)%09%(upstream:track)",
            "refs/heads",
        ],
    )?;
    let mut names = Vec::new();
    let mut tracking = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut cols = line.split('\t');
        let Some(name) = cols
            .next()
            .and_then(|r| r.trim().strip_prefix("refs/heads/"))
        else {
            continue;
        };
        names.push(name.to_string());
        let upstream_ref = cols.next().unwrap_or("").trim();
        if upstream_ref.is_empty() {
            continue; // 无 upstream 的分支不收录
        }
        let track = cols.next().unwrap_or("");
        if track.trim() == "[gone]" {
            tracking.push(GitBranchTrack {
                name: name.to_string(),
                upstream: None,
                ahead: 0,
                behind: 0,
            });
            continue;
        }
        let upstream = upstream_ref
            .strip_prefix("refs/remotes/")
            .or_else(|| upstream_ref.strip_prefix("refs/heads/"))
            .unwrap_or(upstream_ref);
        let (ahead, behind) = parse_upstream_track(track);
        tracking.push(GitBranchTrack {
            name: name.to_string(),
            upstream: Some(upstream.to_string()),
            ahead,
            behind,
        });
    }
    Ok((names, tracking))
}

#[tauri::command]
pub async fn list_git_branches(path: String) -> AppResult<GitBranches> {
    run_blocking(move || list_branches_blocking(&path)).await
}

fn list_branches_blocking(path: &str) -> AppResult<GitBranches> {
    // 远程分支:附带 symref 列(tab 分隔),过滤掉 origin/HEAD 这类符号引用;
    // 名称取完整 refname 剥 refs/remotes/ 前缀,与 git log %D 装饰命名一致
    // (不用 %(refname:short),它与本地分支同名 remote 歧义时会输出 remotes/zc 等消歧形式)
    let remote_out = run_git(
        path,
        &["for-each-ref", "--format=%(refname)%09%(symref)", "refs/remotes"],
    )?;
    let remote = String::from_utf8_lossy(&remote_out.stdout)
        .lines()
        .filter_map(|l| {
            let (name, symref) = l.split_once('\t').unwrap_or((l, ""));
            let short = name.strip_prefix("refs/remotes/").unwrap_or(name);
            if short.is_empty() || !symref.is_empty() {
                None
            } else {
                Some(short.to_string())
            }
        })
        .collect();
    let (local, tracking) = local_branches_with_tracking(path)?;
    Ok(GitBranches {
        local,
        remote,
        tracking,
    })
}

/// 在项目目录初始化 git 仓库,返回最新状态。
/// branch 为初始分支名(空回退 main);`git init -b` 需要 git 2.28+,
/// 旧版本回退到不带 -b 的 init 后用 `checkout -b` 改名未出生分支。
/// remote_url 非空时将其添加为 origin;失败返回错误,但 init 幂等,
/// 用户修正后重试不会产生副作用
#[tauri::command]
pub async fn git_init(
    path: String,
    branch: String,
    remote_url: Option<String>,
) -> AppResult<GitStatus> {
    run_blocking(move || {
        let branch = {
            let b = branch.trim();
            if b.is_empty() { "main" } else { b }
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
        if let Some(url) = remote_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            run_git(&path, &["remote", "add", "origin", url])?;
        }
        let st = status(&path)?;
        cache_status(&path, &st);
        Ok(st)
    })
    .await
}

/// 切换分支;create 为 true 时创建并切换(`git checkout -b`),
/// start_point 非空时以其为基点创建(可为本地分支或 origin/xxx 形式的远程分支)。
/// remote 为 true 时 branch 形如 "origin/feature":本地已有同名分支则直接切换,
/// 否则创建跟踪分支(`git checkout -b feature --track origin/feature`)
#[tauri::command]
pub async fn git_checkout(
    path: String,
    branch: String,
    create: bool,
    remote: bool,
    start_point: Option<String>,
) -> AppResult<GitStatus> {
    run_blocking(move || checkout_blocking(&path, &branch, create, remote, start_point.as_deref()))
        .await
}

fn checkout_blocking(
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
fn is_nested_repo(path: &str, entry: &str) -> bool {
    entry.ends_with('/')
        && Path::new(path)
            .join(entry.trim_end_matches('/'))
            .join(".git")
            .exists()
}

/// 在当前调用内缓存已确认的嵌套仓库目录。
/// 仅缓存命中项,避免重复检查同一目录的 `.git` 路径。
fn is_nested_repo_cached(
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
fn nested_repo_dirs(path: &str) -> Vec<String> {
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
/// 嵌套 git 仓库始终排除,避免被误加成 embedded gitlink(只存 commit 指针)
#[tauri::command]
pub async fn git_commit(
    path: String,
    message: String,
    include_untracked: bool,
) -> AppResult<GitStatus> {
    run_blocking(move || commit_blocking(&path, &message, include_untracked)).await
}

fn commit_blocking(path: &str, message: &str, include_untracked: bool) -> AppResult<GitStatus> {
    let message = message.trim();
    if message.is_empty() {
        return Err(AppError::coded(ErrorCode::GitCommitMessageRequired, ""));
    }
    if include_untracked {
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
    } else {
        run_git(path, &["add", "-u"])?;
    }
    run_git(path, &["commit", "-m", message])?;
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

/// 拉取远端。产生合并冲突时不算失败:返回冲突文件列表,由前端引导用户解决。
/// branch 指定拉取目标分支:为当前检出分支(或缺省)时走 `git pull`;
/// 为其他本地分支时不切换工作区,经 `git fetch <remote> <src>:<branch>` 快进更新引用
/// (分叉或分支被其他 worktree 占用时由 git 报错透传)
#[tauri::command]
pub async fn git_pull(path: String, branch: Option<String>) -> AppResult<GitPullResult> {
    run_blocking(move || match branch {
        Some(b) if !b.is_empty() && current_branch(&path).as_deref() != Some(b.as_str()) => {
            pull_branch_blocking(&path, &b)
        }
        _ => pull_blocking(&path),
    })
    .await
}

fn pull_blocking(path: &str) -> AppResult<GitPullResult> {
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
fn pull_branch_blocking(path: &str, branch: &str) -> AppResult<GitPullResult> {
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
fn current_branch(path: &str) -> Option<String> {
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
fn upstream_of(path: &str, branch: &str) -> Option<(String, String)> {
    let out = git_command(path)
        .args(["rev-parse", "--abbrev-ref", &format!("{branch}@{{upstream}}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    split_remote_branch(String::from_utf8_lossy(&out.stdout).trim())
}

/// 将 "origin/feature/x" 拆为 ("origin", "feature/x");不含 '/' 时无法判定远端,返回 None
fn split_remote_branch(name: &str) -> Option<(String, String)> {
    let (remote, branch) = name.split_once('/')?;
    if remote.is_empty() || branch.is_empty() {
        return None;
    }
    Some((remote.to_string(), branch.to_string()))
}

/// 首推回退时的目标远端:优先 origin,否则取列表第一个远端;
/// 一个都没有返回 None(此时 `git push` 会先报无推送目标,走不到这里)
fn default_push_remote(path: &str) -> Option<String> {
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
pub async fn git_push(path: String, branch: Option<String>) -> AppResult<GitStatus> {
    run_blocking(move || match branch {
        Some(b) if !b.is_empty() && current_branch(&path).as_deref() != Some(b.as_str()) => {
            push_branch_blocking(&path, &b)
        }
        _ => push_blocking(&path),
    })
    .await
}

/// 推送非当前检出的本地分支(不影响工作区)
fn push_branch_blocking(path: &str, branch: &str) -> AppResult<GitStatus> {
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
    path: String,
    branch: String,
    force: bool,
) -> AppResult<GitStatus> {
    run_blocking(move || branch_delete_blocking(&path, &branch, force)).await
}

fn branch_delete_blocking(path: &str, branch: &str, force: bool) -> AppResult<GitStatus> {
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
pub async fn git_remote_branch_delete(path: String, branch: String) -> AppResult<GitStatus> {
    run_blocking(move || remote_branch_delete_blocking(&path, &branch)).await
}

fn remote_branch_delete_blocking(path: &str, branch: &str) -> AppResult<GitStatus> {
    let (remote, short) = split_remote_branch(branch.trim())
        .ok_or_else(|| AppError::coded(ErrorCode::GitBranchNameRequired, ""))?;
    run_git(path, &["push", &remote, "--delete", &short])?;
    let st = status(path)?;
    cache_status(path, &st);
    Ok(st)
}

fn push_blocking(path: &str) -> AppResult<GitStatus> {
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

// ── worktree / merge / rebase ─────────────────────────────

/// 解析 `git worktree list --porcelain` 输出。
/// 块格式:`worktree <path>` 起始,后跟 `HEAD <sha>`、`branch refs/heads/<name>` 或 `detached`;
/// 第一条记录为主工作区
fn parse_worktree_porcelain(text: &str) -> Vec<GitWorktree> {
    let mut list = Vec::new();
    let mut cur: Option<GitWorktree> = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(w) = cur.take() {
                list.push(w);
            }
            cur = Some(GitWorktree {
                path: p.trim().to_string(),
                branch: None,
                head: String::new(),
                is_main: false,
                detached: false,
            });
        } else if let Some(w) = cur.as_mut() {
            if let Some(h) = line.strip_prefix("HEAD ") {
                w.head = h.trim().to_string();
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                w.branch = Some(b.trim().to_string());
            } else if line.trim() == "detached" {
                w.detached = true;
            }
        }
    }
    if let Some(w) = cur.take() {
        list.push(w);
    }
    if let Some(first) = list.first_mut() {
        first.is_main = true;
    }
    list
}

fn list_worktrees_blocking(path: &str) -> AppResult<Vec<GitWorktree>> {
    let out = run_git(path, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_porcelain(&String::from_utf8_lossy(
        &out.stdout,
    )))
}

#[tauri::command]
pub async fn list_git_worktrees(path: String) -> AppResult<Vec<GitWorktree>> {
    run_blocking(move || list_worktrees_blocking(&path)).await
}

/// worktree 目标目录:`{branch}` 占位符替换为分支名(`/` 转 `-`,避免多级路径);
/// 相对路径基于主工作区根解析,绝对路径原样使用
fn resolve_worktree_target(main_root: &str, input: &str, branch: &str) -> PathBuf {
    let templated = input.replace("{branch}", &branch.replace('/', "-"));
    let p = Path::new(&templated);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(main_root).join(p)
    }
}

/// 创建 worktree。create_branch 为 true 时检出新分支
/// (`git worktree add <dir> -b <branch> [start_point]`,start_point 缺省为 HEAD);
/// 为 false 时挂载已有分支:本地分支直接挂载;origin/xxx 远程引用在本地无同名分支时
/// 显式创建跟踪分支(直接传 origin/x 只会得到游离 HEAD,不触发 checkout 式 DWIM),
/// 本地已有同名分支时按安全快进对齐(见 attach_remote_worktree)。
/// 分支已被其它 worktree 检出时报 git_branch_checked_out
#[tauri::command]
pub async fn git_worktree_add(
    path: String,
    worktree_path: String,
    branch: String,
    create_branch: bool,
    start_point: Option<String>,
) -> AppResult<Vec<GitWorktree>> {
    run_blocking(move || {
        worktree_add_blocking(
            &path,
            &worktree_path,
            &branch,
            create_branch,
            start_point.as_deref(),
        )
    })
    .await
}

fn worktree_add_blocking(
    path: &str,
    worktree_path: &str,
    branch: &str,
    create_branch: bool,
    start_point: Option<&str>,
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
    if existing.iter().any(|w| w.branch.as_deref() == Some(local_name)) {
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
    list_worktrees_blocking(path)
}

/// 挂载远程引用(origin/x)到 worktree。
/// - 本地无同名分支:`git worktree add --track -b x <dir> origin/x` 显式建跟踪分支
/// - 本地已有同名分支且可能不同步:本地落后/持平时先 `git branch -f` 对齐到远程提交
///   再挂载;本地领先(远程是其祖先)直接挂载本地分支;真正分叉时报
///   git_branch_diverged —— 不静默重置分支,避免本地未推送提交从分支上丢失
fn attach_remote_worktree(
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
            &["worktree", "add", "--track", "-b", local_name, target, remote],
        )?;
    }
    Ok(())
}

/// `git merge-base --is-ancestor a b`:a 是否为 b 的祖先(0=是,1=否,其它视为命令失败)
fn is_ancestor(path: &str, a: &str, b: &str) -> AppResult<bool> {
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
/// (force 时用 -D,否则 -d 安全删除;未合并的分支会因 -d 报错,此时 worktree 已删,
/// 用户可勾选强制重试)。主工作区不可删除。返回最新 worktree 列表
#[tauri::command]
pub async fn git_worktree_remove(
    path: String,
    worktree_path: String,
    force: bool,
    delete_branch: bool,
) -> AppResult<Vec<GitWorktree>> {
    run_blocking(move || worktree_remove_blocking(&path, &worktree_path, force, delete_branch))
        .await
}

fn worktree_remove_blocking(
    path: &str,
    worktree_path: &str,
    force: bool,
    delete_branch: bool,
) -> AppResult<Vec<GitWorktree>> {
    let existing = list_worktrees_blocking(path)?;
    let target = existing
        .iter()
        .find(|w| w.path == worktree_path)
        .ok_or_else(|| AppError::coded(ErrorCode::InvalidPath, worktree_path))?;
    if target.is_main {
        return Err(AppError::coded(
            ErrorCode::GitCommandFailed,
            "cannot remove main worktree",
        ));
    }
    let branch = target.branch.clone();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path);
    run_git(path, &args)?;
    if delete_branch {
        if let Some(b) = branch {
            let flag = if force { "-D" } else { "-d" };
            run_git(path, &["branch", flag, &b])?;
        }
    }
    list_worktrees_blocking(path)
}

/// 将指定分支合并进当前分支;squash 时只暂存不自动提交(由用户确认后手动提交)。
/// 与 pull 一致:产生冲突不算失败,返回冲突文件列表由前端引导解决
#[tauri::command]
pub async fn git_merge(path: String, branch: String, squash: bool) -> AppResult<GitMergeResult> {
    run_blocking(move || merge_blocking(&path, &branch, squash)).await
}

fn merge_blocking(path: &str, branch: &str, squash: bool) -> AppResult<GitMergeResult> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    let args: Vec<&str> = if squash {
        vec!["merge", "--squash", branch]
    } else {
        vec!["merge", branch]
    };
    let result = git_command(path).args(&args).output()?;
    let conflicts = unmerged_files(path);
    if !result.status.success() && conflicts.is_empty() {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(if detail.is_empty() {
            AppError::coded(ErrorCode::GitCommandFailed, "")
        } else {
            friendly_git_error(&detail)
        });
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(GitMergeResult {
        status: st,
        conflicts,
    })
}

/// 中止进行中的合并(`git merge --abort`),返回最新状态
#[tauri::command]
pub async fn git_merge_abort(path: String) -> AppResult<GitStatus> {
    run_blocking(move || {
        run_git(&path, &["merge", "--abort"])?;
        let st = status(&path)?;
        cache_status(&path, &st);
        Ok(st)
    })
    .await
}

/// 变基是否处于中断状态:git dir 下存在 rebase-merge / rebase-apply 目录。
/// 用 --absolute-git-dir 兼容 worktree(其 .git 是指向主仓库 gitdir 的文件)
fn rebase_in_progress(path: &str) -> bool {
    let Ok(out) = run_git(path, &["rev-parse", "--absolute-git-dir"]) else {
        return false;
    };
    let gitdir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if gitdir.is_empty() {
        return false;
    }
    let gd = Path::new(&gitdir);
    gd.join("rebase-merge").exists() || gd.join("rebase-apply").exists()
}

/// 将当前分支变基到 onto 之上。冲突/中断不算失败:返回冲突文件与 in_progress,
/// 由前端引导用户外部解决后 --continue,或调用 git_rebase_abort 中止
#[tauri::command]
pub async fn git_rebase(path: String, onto: String) -> AppResult<GitRebaseResult> {
    run_blocking(move || rebase_blocking(&path, &onto)).await
}

fn rebase_blocking(path: &str, onto: &str) -> AppResult<GitRebaseResult> {
    let onto = onto.trim();
    if onto.is_empty() {
        return Err(AppError::coded(ErrorCode::GitBranchNameRequired, ""));
    }
    let result = git_command(path).args(["rebase", onto]).output()?;
    let conflicts = unmerged_files(path);
    let in_progress = rebase_in_progress(path);
    if !result.status.success() && conflicts.is_empty() && !in_progress {
        let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(if detail.is_empty() {
            AppError::coded(ErrorCode::GitCommandFailed, "")
        } else {
            friendly_git_error(&detail)
        });
    }
    let st = status(path)?;
    cache_status(path, &st);
    Ok(GitRebaseResult {
        status: st,
        conflicts,
        in_progress,
    })
}

/// 中止进行中的变基(`git rebase --abort`),返回最新状态
#[tauri::command]
pub async fn git_rebase_abort(path: String) -> AppResult<GitStatus> {
    run_blocking(move || {
        run_git(&path, &["rebase", "--abort"])?;
        let st = status(&path)?;
        cache_status(&path, &st);
        Ok(st)
    })
    .await
}
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

/// 按 char 边界安全截断,返回 (文本, 是否截断)
fn truncate_chars(text: &str, max: usize) -> (String, bool) {
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
#[tauri::command]
pub async fn git_commit_context(path: String) -> AppResult<GitCommitContext> {
    run_blocking(move || commit_context_blocking(&path)).await
}

fn commit_context_blocking(path: &str) -> AppResult<GitCommitContext> {
    let has_head = git_command(path)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let base: &[&str] = if has_head { &["HEAD"] } else { &["--cached"] };

    let stat_out = run_git(path, &[&["diff", "--stat"], base].concat())?;
    let diff_out = run_git(
        path,
        &[&["diff"], base, &["--", "."], DIFF_EXCLUDES].concat(),
    )?;

    let untracked_out = run_git(path, &["ls-files", "--others", "--exclude-standard"])?;
    // 缓存本次扫描中已确认的嵌套仓库
    let mut nested_cache: std::collections::HashSet<String> = std::collections::HashSet::new();
    let untracked: Vec<String> = String::from_utf8_lossy(&untracked_out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !is_nested_repo_cached(path, l, &mut nested_cache))
        .map(String::from)
        .collect();

    let mut untracked_files = Vec::new();
    let mut budget = UNTRACKED_TOTAL_MAX_CHARS;
    for name in &untracked {
        if budget == 0 {
            break;
        }
        if let Some(mut f) = read_untracked_file(path, name) {
            let (content, hit_budget) = truncate_chars(&f.content, budget);
            f.truncated = f.truncated || hit_budget;
            budget -= content.chars().count();
            f.content = content;
            untracked_files.push(f);
        }
    }

    let recent_commits = if has_head {
        run_git(
            path,
            &[
                "log",
                "--no-merges",
                &format!("-{RECENT_COMMITS_COUNT}"),
                "--pretty=%s",
            ],
        )
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    let (diff, truncated) =
        truncate_chars(&String::from_utf8_lossy(&diff_out.stdout), DIFF_MAX_CHARS);
    Ok(GitCommitContext {
        stat: String::from_utf8_lossy(&stat_out.stdout).trim().to_string(),
        diff,
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

/// "良性空结果"的 git log stderr 特征(非仓库/无提交/坏默认分支)
fn is_benign_log_stderr(stderr: &str) -> bool {
    stderr.contains("not a git repository")
        || stderr.contains("does not have any commits")
        || stderr.contains("your current branch")
        || stderr.contains("bad default revision")
}

/// 执行 git log 类命令并处理"良性空结果"(非仓库/无提交/坏默认分支 → Ok(None))
fn run_git_log_raw(path: &str, args: &[String]) -> AppResult<Option<Output>> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = git_command(path).args(&arg_refs).output()?;
    if output.status.success() {
        return Ok(Some(output));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_benign_log_stderr(&stderr) {
        return Ok(None);
    }
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        AppError::coded(ErrorCode::GitLogFailed, output.status.to_string())
    } else {
        friendly_git_error(detail)
    })
}

/// git_log 核心逻辑,供 scheduler 等内部模块复用;参数均为引用以避免不必要的 clone
pub(crate) fn run_git_log(
    path: &str,
    since: Option<&str>,
    until: Option<&str>,
    max_count: Option<u32>,
    author: Option<&str>,
) -> AppResult<Vec<GitCommitInfo>> {
    let mut args: Vec<String> = vec![
        "log".into(),
        "--no-merges".into(),
        "--pretty=format:%h%x1f%an%x1f%ad%x1f%s".into(),
        "--date=format:%Y-%m-%d %H:%M".into(),
    ];
    if let Some(s) = since.filter(|s| !s.trim().is_empty()) {
        args.push(format!("--since={s}"));
    }
    if let Some(u) = until.filter(|u| !u.trim().is_empty()) {
        args.push(format!("--until={u}"));
    }
    if let Some(a) = author.filter(|a| !a.trim().is_empty()) {
        args.push(format!("--author={a}"));
    }
    let limit = max_count.unwrap_or(200).min(1000);
    args.push(format!("--max-count={limit}"));

    let Some(output) = run_git_log_raw(path, &args)? else {
        return Ok(Vec::new());
    };

    let commits = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '\x1f');
            let hash = parts.next()?.trim();
            let author = parts.next()?.trim();
            let date = parts.next()?.trim();
            let subject = parts.next()?.trim();
            if hash.is_empty() {
                return None;
            }
            Some(GitCommitInfo {
                hash: hash.to_string(),
                author: author.to_string(),
                date: date.to_string(),
                subject: subject.to_string(),
            })
        })
        .collect();
    Ok(commits)
}

/// 读取提交图谱数据(含合并提交与引用装饰),按拓扑序流式输出,支持全量历史。
/// --topo-order 保证子提交先于父提交,是前端泳道布局的前提;
/// 非 git 仓库或尚无提交时仅推送一个 done 批次而非报错。
/// 修订范围:branches 非空时按指定分支(本地或 origin/xxx)取日志;
/// 否则 include_remote 为 false 时仅本地分支+标签(--branches --tags),默认 --all 含远程。
/// 结果按 batch_size 分批经 channel 推送(单次 git walk 边读边发),最后一批 done = true
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
        let revs: Vec<String> = match branches {
            Some(list) => list
                .into_iter()
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect(),
            None if include_remote == Some(false) => {
                vec!["--branches".into(), "--tags".into()]
            }
            None => vec!["--all".into()],
        };
        if revs.is_empty() {
            let _ = on_batch.send(GitGraphBatch {
                commits: Vec::new(),
                done: true,
            });
            return Ok(());
        }
        let mut args: Vec<String> = vec![
            "log".into(),
            "--topo-order".into(),
            "--pretty=format:%H%x1f%P%x1f%an%x1f%ad%x1f%s%x1f%D".into(),
            "--date=format:%Y-%m-%d %H:%M".into(),
        ];
        args.extend(revs);

        let mut child = git_command(&path)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::coded(ErrorCode::GitLogFailed, e.to_string()))?;
        let stdout = child.stdout.take().expect("stdout 已通过管道捕获");
        let mut batch: Vec<GitGraphCommit> = Vec::with_capacity(size);
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| AppError::coded(ErrorCode::GitLogFailed, e.to_string()))?;
            if let Some(commit) = parse_graph_commit_line(&line) {
                batch.push(commit);
                if batch.len() >= size {
                    let _ = on_batch.send(GitGraphBatch {
                        commits: std::mem::take(&mut batch),
                        done: false,
                    });
                }
            }
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if batch.is_empty() && is_benign_log_stderr(&stderr) {
                let _ = on_batch.send(GitGraphBatch {
                    commits: Vec::new(),
                    done: true,
                });
                return Ok(());
            }
            let detail = stderr.trim();
            return Err(if detail.is_empty() {
                AppError::coded(ErrorCode::GitLogFailed, output.status.to_string())
            } else {
                friendly_git_error(detail)
            });
        }
        let _ = on_batch.send(GitGraphBatch {
            commits: batch,
            done: true,
        });
        Ok(())
    })
    .await
}

/// 解析 %H%x1f%P%x1f%an%x1f%ad%x1f%s%x1f%D 格式的一行 git log 输出
fn parse_graph_commit_line(line: &str) -> Option<GitGraphCommit> {
    let mut parts = line.splitn(6, '\x1f');
    let hash = parts.next()?.trim();
    let parents = parts.next()?.trim();
    let author = parts.next()?.trim();
    let date = parts.next()?.trim();
    let subject = parts.next()?.trim();
    let decorations = parts.next().unwrap_or("").trim();
    if hash.is_empty() {
        return None;
    }
    let mut refs = Vec::new();
    let mut is_head = false;
    for deco in decorations.split(", ").map(str::trim).filter(|d| !d.is_empty()) {
        if let Some(target) = deco.strip_prefix("HEAD -> ") {
            is_head = true;
            refs.push(target.to_string());
        } else if deco == "HEAD" {
            is_head = true;
        } else {
            refs.push(deco.to_string());
        }
    }
    Some(GitGraphCommit {
        hash: hash.to_string(),
        parents: parents.split_whitespace().map(str::to_string).collect(),
        author: author.to_string(),
        date: date.to_string(),
        subject: subject.to_string(),
        refs,
        is_head,
    })
}

/// 提交详情面板单文件 diff 的长度上限(超出截断,避免大文件撑爆 IPC)
const COMMIT_DIFF_MAX_CHARS: usize = 200_000;

/// 读取某次提交触及的文件清单(状态 + 增删行数,提交详情面板文件列表用)。
/// diff-tree --root 兼容根提交(相对空树,全部视为新增);-M 识别重命名;
/// --numstat 与 --name-status 两个输出块文件顺序一致,解析后按索引配对。
/// 合并提交(多父)diff-tree 默认无输出,返回空数组由前端提示
#[tauri::command]
pub async fn git_commit_files(path: String, hash: String) -> AppResult<Vec<GitCommitFile>> {
    run_blocking(move || {
        let out = run_git(
            &path,
            &[
                "-c",
                "core.quotepath=false",
                "diff-tree",
                "--root",
                "-r",
                "-M",
                "--no-commit-id",
                "--numstat",
                "--name-status",
                &hash,
            ],
        )?;
        Ok(parse_commit_files(&String::from_utf8_lossy(&out.stdout)))
    })
    .await
}

/// 解析 `diff-tree --numstat --name-status` 的组合输出。
/// 输出为两块(numstat 在前、name-status 在后,空行分隔),按行首列区分:
/// numstat 行首列是数字或 "-"(二进制),name-status 行首列是状态字母。
/// 两块文件顺序一致,按索引把增删行数配到状态条目上
fn parse_commit_files(text: &str) -> Vec<GitCommitFile> {
    let mut stats: Vec<(Option<u32>, Option<u32>)> = Vec::new();
    let mut files: Vec<GitCommitFile> = Vec::new();
    for line in text.lines() {
        let mut cols = line.split('\t');
        let Some(first) = cols.next() else { continue };
        if first.is_empty() {
            continue;
        }
        if first.as_bytes()[0].is_ascii_digit() || first == "-" {
            // numstat 行:"-" 表示二进制文件,行数记 None
            stats.push((
                first.parse::<u32>().ok(),
                cols.next().and_then(|s| s.parse::<u32>().ok()),
            ));
            continue;
        }
        let status = first.chars().next().unwrap_or('\0');
        if !"ACDMRT".contains(status) {
            continue;
        }
        let p1 = cols.next().unwrap_or("").to_string();
        let p2 = cols.next().map(str::to_string);
        // 重命名 / 复制(R100 / C123):后随旧、新两个路径
        let (old_path, path) = if matches!(status, 'R' | 'C') {
            (Some(p1), p2.unwrap_or_default())
        } else {
            (None, p1)
        };
        if path.is_empty() {
            continue;
        }
        files.push(GitCommitFile {
            path,
            old_path,
            status: status.to_string(),
            additions: None,
            deletions: None,
        });
    }
    for (i, file) in files.iter_mut().enumerate() {
        if let Some((adds, dels)) = stats.get(i) {
            file.additions = *adds;
            file.deletions = *dels;
        }
    }
    files
}

/// 读取某次提交中单个文件的 diff(提交详情面板用)。
/// 用 `git show --format=` 而非 `diff <hash>^ <hash>`:根提交无父提交,前者天然兼容;
/// 重命名时新旧路径都作为 pathspec 传入。超长按字符截断(二进制 diff 天然很短)
#[tauri::command]
pub async fn git_commit_file_diff(
    path: String,
    hash: String,
    file_path: String,
    old_path: Option<String>,
) -> AppResult<GitCommitFileDiff> {
    run_blocking(move || {
        let mut args: Vec<&str> =
            vec!["-c", "core.quotepath=false", "show", "--format=", &hash, "--"];
        if let Some(old) = old_path.as_deref() {
            args.push(old);
        }
        args.push(&file_path);
        let out = run_git(&path, &args)?;
        let (diff, truncated) =
            truncate_chars(&String::from_utf8_lossy(&out.stdout), COMMIT_DIFF_MAX_CHARS);
        Ok(GitCommitFileDiff { diff, truncated })
    })
    .await
}

/// 读取仓库当前 git 用户身份(日报"仅自己"过滤用)。
/// `git config user.name/email` 本身含全局配置回退;非仓库或未配置时返回空串而非报错
#[tauri::command]
pub async fn git_current_user(path: String) -> AppResult<GitUser> {
    run_blocking(move || run_git_current_user(&path)).await
}

pub(crate) fn run_git_current_user(path: &str) -> AppResult<GitUser> {
    let read = |key: &str| -> String {
        git_command(path)
            .args(["config", key])
            .output()
            .map(|o| {
                if o.status.success() {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                } else {
                    String::new()
                }
            })
            .unwrap_or_default()
    };
    Ok(GitUser {
        name: read("user.name"),
        email: read("user.email"),
    })
}

/// 删除目录(带重试)。取消克隆时 Windows 上被杀掉的子进程可能短暂持有
/// 文件句柄,立即 remove_dir_all 会失败,故重试几次
async fn remove_dir_all_retry(path: &Path) -> std::io::Result<()> {
    for attempt in 0..5 {
        match tokio::fs::remove_dir_all(path).await {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if attempt == 4 => return Err(e),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }
    Ok(())
}

/// 克隆仓库到本地目录,返回克隆后的路径。
/// 期间可通过 cancel_git_clone(job_id) 中断;失败/取消都会清理半成品目录。
/// 进度行刷在 stderr 但不透传(前端仅 loading),只保留末尾用于错误提示。
/// account_id 传入时(从「账号仓库」入口克隆)用绑定账号的 token 拼认证 URL 克隆,
/// 成功后把 origin 重置为干净 URL,避免 token 残留在 .git/config
#[tauri::command]
pub async fn git_clone(
    db: State<'_, Db>,
    url: String,
    target_path: String,
    job_id: String,
    account_id: Option<i64>,
) -> AppResult<String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::coded(ErrorCode::GitCloneUrlRequired, ""));
    }
    // 账号凭据拼进 clone URL(仅 http(s) 地址生效,ssh 地址原样使用)
    let clone_url = match account_id {
        // GitHub CLI 虚拟账号:不查库,token 取自 gh(必须先于查库分支)
        Some(id) if id == account::GH_CLI_ACCOUNT_ID => {
            let (provider, username, token) = account::gh_cli_git_credentials().await?;
            account::build_authed_url(&provider, &username, &token, &url)
        }
        Some(id) => {
            let (provider, username, token) = {
                let conn = db.0.lock().unwrap();
                account::get_credentials(&conn, id)?
            };
            account::build_authed_url(&provider, &username, &token, &url)
        }
        None => url.clone(),
    };
    let target = Path::new(&target_path);
    let parent = target
        .parent()
        .ok_or_else(|| AppError::coded(ErrorCode::GitCloneInvalidTarget, target_path.clone()))?;
    if !parent.is_dir() {
        return Err(AppError::coded(
            ErrorCode::GitCloneParentMissing,
            parent.display().to_string(),
        ));
    }
    if target.exists() {
        return Err(AppError::coded(ErrorCode::GitCloneTargetExists, target_path.clone()));
    }

    let mut command = tokio::process::Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    // 用账号 token 克隆时禁用凭据助手:认证只走 URL 内嵌的 token,
    // 避免 GCM 把 token 存进系统凭据管理器;后续 pull/push 由用户自己的凭据解决
    if clone_url != url {
        command.arg("-c").arg("credential.helper=");
    }
    command
        .args(["clone", "--", &clone_url, &target_path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command
        .spawn()
        .map_err(|e| AppError::coded(ErrorCode::GitCloneSpawnFailed, e.to_string()))?;
    // 登记 PID 供应用退出钩子按 PID 清理(child 随后 move 进 CLONE_JOBS,
    // 但 pid 已拷出为独立副本,不受句柄所有权转移影响)
    let _tracked = TrackedPid::new(child.id());

    // stderr 由独立任务持续消费,避免管道写满阻塞子进程;
    // 只保留末尾 8KB(进度行很长,且只需末尾的失败原因)
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tauri::async_runtime::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut text = buf.lock().unwrap();
                        text.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        if text.len() > 8192 {
                            let mut cut = text.len() - 4096;
                            while !text.is_char_boundary(cut) {
                                cut += 1;
                            }
                            text.drain(..cut);
                        }
                    }
                }
            }
        });
    }

    clone_jobs().lock().await.insert(job_id.clone(), child);

    // 轮询等待结束;注册表项被 cancel_git_clone 移除即视为用户取消
    let result: AppResult<()> = loop {
        let polled = {
            let mut jobs = clone_jobs().lock().await;
            match jobs.get_mut(&job_id) {
                None => break Err(AppError::coded(ErrorCode::GitCloneCanceled, "")),
                Some(child) => child.try_wait(),
            }
        };
        match polled {
            Ok(Some(status)) if status.success() => {
                clone_jobs().lock().await.remove(&job_id);
                break Ok(());
            }
            Ok(Some(_)) => {
                clone_jobs().lock().await.remove(&job_id);
                let detail = stderr_buf.lock().unwrap().trim().to_string();
                break Err(if detail.is_empty() {
                    AppError::coded(ErrorCode::GitCloneFailed, "")
                } else {
                    friendly_git_error(&detail)
                });
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
            Err(e) => {
                clone_jobs().lock().await.remove(&job_id);
                break Err(AppError::coded(ErrorCode::GitClonePollFailed, e.to_string()));
            }
        }
    };

    // 失败/取消时清理半成品目录(取消场景子进程刚被 kill,句柄释放有延迟,靠重试覆盖)
    if result.is_err() && target.exists() {
        let _ = remove_dir_all_retry(target).await;
    }
    // 用账号凭据克隆成功后,把 origin 重置为干净 URL(token 不留在 .git/config)
    if result.is_ok() && clone_url != url {
        let _ = run_git(&target_path, &["remote", "set-url", "origin", &url]);
    }
    result.map(|()| target_path)
}

/// 列出所有未归档项目的 origin 地址(非仓库/无 remote 的项目跳过),
/// 供前端与账号仓库列表做「已添加」匹配
#[tauri::command]
pub async fn list_project_remote_urls(db: State<'_, Db>) -> AppResult<Vec<String>> {
    let paths = {
        let conn = db.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT path FROM projects WHERE archived_at IS NULL")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    run_blocking(move || {
        let mut urls = Vec::new();
        for path in paths {
            if let Ok(o) = git_command(&path)
                .args(["remote", "get-url", "origin"])
                .output()
            {
                if o.status.success() {
                    let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !url.is_empty() {
                        urls.push(url);
                    }
                }
            }
        }
        Ok(urls)
    })
    .await
}

/// 取消进行中的克隆:kill 子进程并从注册表移除(git_clone 轮询发现后清理目录)。
/// Windows 上用 taskkill /T 杀整棵进程树(clone 会派生 remote helper 孙进程)
#[tauri::command]
pub async fn cancel_git_clone(job_id: String) -> AppResult<()> {
    let child = clone_jobs().lock().await.remove(&job_id);
    if let Some(mut child) = child {
        #[cfg(windows)]
        if let Some(pid) = child.id() {
            let mut cmd = std::process::Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let _ = cmd.output();
        }
        // 非 Windows 主路径;Windows 上作为 taskkill 的兜底(重复 kill 无害)
        let _ = child.start_kill();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-git-test-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(dir: &PathBuf, args: &[&str]) {
        let out = git_command(dir.to_str().unwrap())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} 失败: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &PathBuf) {
        git(dir, &["init", "-b", "main"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "test"]);
    }

    #[test]
    fn parse_commit_files_pairs_numstat_and_name_status() {
        let text = "10\t2\tsrc/foo.ts\n3\t1\tsrc/bar.vue\n-\t-\tassets/logo.png\n\nM\tsrc/foo.ts\nD\tsrc/bar.vue\nA\tassets/logo.png\n";
        let files = parse_commit_files(text);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].status, "M");
        assert_eq!(files[0].path, "src/foo.ts");
        assert_eq!(files[0].additions, Some(10));
        assert_eq!(files[0].deletions, Some(2));
        assert_eq!(files[1].status, "D");
        assert_eq!(files[1].additions, Some(3));
        // 二进制:numstat 为 "-",行数为 None
        assert_eq!(files[2].status, "A");
        assert_eq!(files[2].additions, None);
        assert_eq!(files[2].deletions, None);
    }

    #[test]
    fn parse_commit_files_handles_rename_and_garbage() {
        let text = "5\t0\tsrc/new.ts\n\nR100\tsrc/old.ts\tsrc/new.ts\nX\n\t\n";
        let files = parse_commit_files(text);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, "R");
        assert_eq!(files[0].old_path.as_deref(), Some("src/old.ts"));
        assert_eq!(files[0].path, "src/new.ts");
        assert_eq!(files[0].additions, Some(5));
        assert!(parse_commit_files("").is_empty());
    }

    #[test]
    fn parse_remote_verbose_dedups_and_takes_fetch() {
        let stdout = "origin\tgit@github.com:user/repo.git (fetch)\norigin\tgit@github.com:user/repo.git (push)\nupstream\thttps://example.com/a/b.git (fetch)\nupstream\thttps://example.com/a/b.git (push)\n";
        let remotes = parse_remote_verbose(stdout);
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "git@github.com:user/repo.git");
        assert_eq!(remotes[1].name, "upstream");
        assert_eq!(remotes[1].url, "https://example.com/a/b.git");
        // 空输出 / 残缺行
        assert!(parse_remote_verbose("").is_empty());
        assert!(parse_remote_verbose("origin\n").is_empty());
    }

    #[test]
    fn non_repo_returns_is_repo_false() {
        let dir = temp_dir("plain");
        let st = status(dir.to_str().unwrap()).unwrap();
        assert!(!st.is_repo);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn friendly_error_strips_ssh_noise_and_maps_local_changes() {
        let raw = "** WARNING: connection is not using a post-quantum key exchange algorithm.\n\
                   ** This session may be vulnerable to \"store now, decrypt later\" attacks.\n\
                   ** The server may need to be upgraded. See https://openssh.com/pq.html\n\
                   error: Your local changes to the following files would be overwritten by merge:\n\
                   \tpages/yudao/yudao-log/index.md\n\
                   Please commit your changes or stash them before you merge.\n\
                   error: The following untracked working tree files would be overwritten by merge:\n\
                   \tpages/yudao/yudao-log/log-2026.md\n\
                   Please move or remove them before you merge.\n\
                   Aborting";
        let err = friendly_git_error(raw);
        assert!(
            err.is_code(crate::error::ErrorCode::GitLocalChangesConflict),
            "实际输出: {err}"
        );
        let msg = err.to_string();
        assert!(!msg.contains("post-quantum"), "实际输出: {msg}");
        assert!(!msg.contains("Aborting"), "实际输出: {msg}");
    }

    #[test]
    fn friendly_error_maps_untracked_overwritten() {
        let raw = "error: The following untracked working tree files would be overwritten by checkout:\n\
                   \tfoo.txt\n\
                   Please move or remove them before you switch branches.\n\
                   Aborting";
        let err = friendly_git_error(raw);
        assert!(
            err.is_code(crate::error::ErrorCode::GitUntrackedConflict),
            "实际输出: {err}"
        );
    }

    #[test]
    fn friendly_error_keeps_no_upstream_branch_phrase() {
        // push_blocking 依赖该原文短语判断首推回退,映射不得覆盖
        let raw = "fatal: The current branch dev has no upstream branch.";
        let err = friendly_git_error(raw);
        assert_eq!(
            err.code(),
            "git_command_failed",
            "未识别错误应落到 git_command_failed: {err}"
        );
        assert!(
            err.to_string().contains("has no upstream branch"),
            "实际输出: {err}"
        );
    }

    #[test]
    fn friendly_error_maps_common_cases() {
        use crate::error::ErrorCode;
        let cases: &[(&str, ErrorCode)] = &[
            (
                "git@github.com: Permission denied (publickey).",
                ErrorCode::GitSshAuthFailed,
            ),
            (
                "ssh: Could not resolve hostname github.com: Temporary failure in name resolution",
                ErrorCode::GitNetworkDns,
            ),
            (
                "error: failed to push some refs to 'origin'\nhint: Updates were rejected because the tip of your current branch is behind",
                ErrorCode::GitPushRejected,
            ),
            (
                "fatal: not a git repository (or any of the parent directories): .git",
                ErrorCode::NotGitRepository,
            ),
            (
                "remote: Repository not found.",
                ErrorCode::GitRepoNotFound,
            ),
            (
                "fatal: You have divergent branches and need to specify how to reconcile them.",
                ErrorCode::GitDiverged,
            ),
        ];
        for (raw, expected) in cases {
            let err = friendly_git_error(raw);
            assert!(err.is_code(*expected), "输入 {raw:?} 实际输出: {err}");
        }
    }

    #[test]
    fn friendly_error_all_noise_falls_back() {
        let err = friendly_git_error(
            "** WARNING: connection is not using a post-quantum key exchange algorithm.",
        );
        assert!(err.is_code(ErrorCode::GitNoiseFallback), "实际输出: {err}");
        assert_eq!(err.code(), "git_noise_fallback");
    }

    #[test]
    fn friendly_error_maps_remote_branch_gone() {
        // 当前分支上游被删:git pull 的实际输出
        let pull_raw = "Your configuration specifies to merge with the ref 'refs/heads/feature'\n\
                        from the remote, but no such ref was fetched.";
        let err = friendly_git_error(pull_raw);
        assert!(
            err.is_code(ErrorCode::GitRemoteBranchGone),
            "实际输出: {err}"
        );
        // 指定分支拉取/抓取:git fetch origin feature:feature 的实际输出(版本间大小写不一)
        for raw in [
            "fatal: couldn't find remote ref feature",
            "fatal: Couldn't find remote ref feature",
        ] {
            let err = friendly_git_error(raw);
            assert!(
                err.is_code(ErrorCode::GitRemoteBranchGone),
                "输入 {raw:?} 实际输出: {err}"
            );
        }
    }

    #[test]
    fn parses_working_tree_counts() {
        let dir = temp_dir("repo");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        // staged: 新文件 b; modified: a; untracked: c
        fs::write(dir.join("b.txt"), "b").unwrap();
        git(&dir, &["add", "b.txt"]);
        fs::write(dir.join("a.txt"), "changed").unwrap();
        fs::write(dir.join("c.txt"), "c").unwrap();

        let st = status(dir.to_str().unwrap()).unwrap();
        assert!(st.is_repo);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.staged, 1);
        assert_eq!(st.modified, 1);
        assert_eq!(st.untracked, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fetch_reports_remote_ahead() {
        // origin(bare) <- clone_a 推送; clone_b 作为被测项目
        let origin = temp_dir("origin");
        git(&origin, &["init", "--bare", "-b", "main"]);

        let clone_a = temp_dir("clone-a");
        git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_a, &["config", "user.email", "test@example.com"]);
        git(&clone_a, &["config", "user.name", "test"]);
        fs::write(clone_a.join("a.txt"), "a").unwrap();
        git(&clone_a, &["add", "a.txt"]);
        git(&clone_a, &["commit", "-m", "c1"]);
        git(&clone_a, &["push", "-u", "origin", "main"]);

        let clone_b = temp_dir("clone-b");
        git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);

        // clone_a 再推一个提交,clone_b fetch 后 remote 领先 1
        fs::write(clone_a.join("a.txt"), "a2").unwrap();
        git(&clone_a, &["commit", "-am", "c2"]);
        git(&clone_a, &["push"]);

        let st = fetch_and_status(clone_b.to_str().unwrap()).unwrap();
        assert!(st.is_repo);
        assert_eq!(st.remote_ahead, 1);
        assert!(st.last_fetch_at.is_none());

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn commit_stages_all_and_cleans_worktree() {
        let dir = temp_dir("commit");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();

        let st = commit_blocking(dir.to_str().unwrap(), "init", true).unwrap();
        assert!(st.is_repo);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.staged, 0);
        assert_eq!(st.modified, 0);
        assert_eq!(st.untracked, 0);

        // 空提交信息被拒绝
        assert!(commit_blocking(dir.to_str().unwrap(), "  ", true).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_untracked_is_opt_in() {
        let dir = temp_dir("commit-untracked");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        fs::write(dir.join("a.txt"), "changed").unwrap(); // 未暂存修改
        fs::write(dir.join("b.txt"), "b").unwrap(); // 未跟踪

        // 不勾选:未暂存修改照常提交,未跟踪文件保留
        let st = commit_blocking(dir.to_str().unwrap(), "tracked only", false).unwrap();
        assert_eq!(st.staged, 0);
        assert_eq!(st.modified, 0);
        assert_eq!(st.untracked, 1);

        // 勾选:未跟踪文件一并提交,工作区干净
        let st = commit_blocking(dir.to_str().unwrap(), "with untracked", true).unwrap();
        assert_eq!(st.staged, 0);
        assert_eq!(st.modified, 0);
        assert_eq!(st.untracked, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn branches_checkout_and_create() {
        let dir = temp_dir("branch");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
        assert_eq!(branches.local, vec!["main".to_string()]);
        assert!(branches.remote.is_empty());

        // 新建并切换
        let st = checkout_blocking(dir.to_str().unwrap(), "feature", true, false, None).unwrap();
        assert_eq!(st.branch.as_deref(), Some("feature"));

        let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
        assert_eq!(
            branches.local,
            vec!["feature".to_string(), "main".to_string()]
        );

        // 切回 main
        let st = checkout_blocking(dir.to_str().unwrap(), "main", false, false, None).unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));

        // 空分支名 / 不存在的分支
        assert!(checkout_blocking(dir.to_str().unwrap(), " ", false, false, None).is_err());
        assert!(checkout_blocking(dir.to_str().unwrap(), "nope", false, false, None).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn branches_keep_log_style_names_when_remote_shares_branch_name() {
        // 分支 zc 与 remote zc 同名时(refs/remotes/zc/HEAD 存在),
        // %(refname:short) 为消歧输出 "heads/zc",而 git log %D 装饰仍显示 "zc";
        // 分支列表必须与 %D 一致,否则图谱侧栏点分支定位顶端提交失败
        let dir = temp_dir("ambiguous-remote");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);
        git(&dir, &["branch", "zc"]);
        let head = git_command(dir.to_str().unwrap())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout;
        let head = String::from_utf8_lossy(&head).trim().to_string();
        git(&dir, &["update-ref", "refs/remotes/zc/HEAD", &head]);
        git(&dir, &["update-ref", "refs/remotes/zc/zc", &head]);

        // 前提校验:git 的 short 命名在此场景下确实会消歧成 heads/zc
        let short = git_command(dir.to_str().unwrap())
            .args(["branch", "--format=%(refname:short)"])
            .output()
            .unwrap()
            .stdout;
        assert!(String::from_utf8_lossy(&short).contains("heads/zc"));

        let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
        assert_eq!(branches.local, vec!["main".to_string(), "zc".to_string()]);
        assert!(branches.remote.contains(&"zc/zc".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkout_remote_creates_tracking_branch() {
        let origin = temp_dir("track-origin");
        git(&origin, &["init", "--bare", "-b", "main"]);

        // clone_a:推 main 和 feature 两个分支到远端
        let clone_a = temp_dir("track-a");
        git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_a, &["config", "user.email", "test@example.com"]);
        git(&clone_a, &["config", "user.name", "test"]);
        fs::write(clone_a.join("a.txt"), "a").unwrap();
        git(&clone_a, &["add", "a.txt"]);
        git(&clone_a, &["commit", "-m", "c1"]);
        git(&clone_a, &["push", "-u", "origin", "main"]);
        git(&clone_a, &["checkout", "-b", "feature"]);
        fs::write(clone_a.join("b.txt"), "b").unwrap();
        git(&clone_a, &["add", "b.txt"]);
        git(&clone_a, &["commit", "-m", "c2"]);
        git(&clone_a, &["push", "-u", "origin", "feature"]);

        let clone_b = temp_dir("track-b");
        git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);

        // 远程分支列出 feature/main,不含 origin/HEAD 符号引用
        let branches = list_branches_blocking(clone_b.to_str().unwrap()).unwrap();
        assert_eq!(branches.local, vec!["main".to_string()]);
        assert_eq!(
            branches.remote,
            vec!["origin/feature".to_string(), "origin/main".to_string()]
        );

        // 检出远程分支:本地无同名分支 → 创建跟踪分支
        let st =
            checkout_blocking(clone_b.to_str().unwrap(), "origin/feature", false, true, None)
                .unwrap();
        assert_eq!(st.branch.as_deref(), Some("feature"));

        // 本地已有同名分支 → 直接切换(幂等,不报错)
        let st =
            checkout_blocking(clone_b.to_str().unwrap(), "origin/feature", false, true, None)
                .unwrap();
        assert_eq!(st.branch.as_deref(), Some("feature"));

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn push_sets_upstream_when_missing() {
        let origin = temp_dir("push-origin");
        git(&origin, &["init", "--bare", "-b", "main"]);

        let clone = temp_dir("push-clone");
        git(&clone, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        git(&clone, &["config", "user.name", "test"]);
        fs::write(clone.join("a.txt"), "a").unwrap();
        git(&clone, &["add", "a.txt"]);
        git(&clone, &["commit", "-m", "c1"]);

        // 首次 push 无 upstream → 自动回退 `git push -u origin HEAD`
        let st = push_blocking(clone.to_str().unwrap()).unwrap();
        assert!(st.is_repo);

        // 已建立 upstream 后走普通 push 路径
        fs::write(clone.join("a.txt"), "a2").unwrap();
        git(&clone, &["commit", "-am", "c2"]);
        push_blocking(clone.to_str().unwrap()).unwrap();

        let out = git_command(origin.to_str().unwrap())
            .args(["rev-list", "--count", "main"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2");

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone);
    }

    #[test]
    fn push_first_time_uses_non_origin_remote() {
        let origin = temp_dir("push-nonorigin");
        git(&origin, &["init", "--bare", "-b", "main"]);

        let clone = temp_dir("push-nonorigin-clone");
        git(&clone, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        git(&clone, &["config", "user.name", "test"]);
        // 远端不叫 origin(如 "github")时,首推回退也应成功
        git(&clone, &["remote", "rename", "origin", "github"]);
        fs::write(clone.join("a.txt"), "a").unwrap();
        git(&clone, &["add", "a.txt"]);
        git(&clone, &["commit", "-m", "c1"]);

        let st = push_blocking(clone.to_str().unwrap()).unwrap();
        assert!(st.is_repo);

        // upstream 应指向 github/main
        let out = git_command(clone.to_str().unwrap())
            .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "github/main");

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone);
    }

    #[test]
    fn split_remote_branch_parses_remote_and_branch() {
        assert_eq!(
            split_remote_branch("origin/feature/x"),
            Some(("origin".to_string(), "feature/x".to_string()))
        );
        assert_eq!(
            split_remote_branch("github/main"),
            Some(("github".to_string(), "main".to_string()))
        );
        assert_eq!(split_remote_branch("main"), None);
        assert_eq!(split_remote_branch("/main"), None);
        assert_eq!(split_remote_branch("origin/"), None);
    }

    fn clone_with_config(tag: &str, origin: &PathBuf) -> PathBuf {
        let dir = temp_dir(tag);
        git(&dir, &["clone", origin.to_str().unwrap(), "."]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "test"]);
        dir
    }

    fn rev_parse(dir: &PathBuf, rev: &str) -> String {
        let out = git_command(dir.to_str().unwrap())
            .args(["rev-parse", rev])
            .output()
            .unwrap();
        assert!(out.status.success(), "rev-parse {rev} 失败");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// origin(bare) + clone_a:首推 main,并创建推送 feature 分支(clone_a 停留在 feature)
    fn setup_origin_with_feature(tag: &str) -> (PathBuf, PathBuf) {
        let origin = temp_dir(&format!("{tag}-origin"));
        git(&origin, &["init", "--bare", "-b", "main"]);
        let clone_a = clone_with_config(&format!("{tag}-a"), &origin);
        fs::write(clone_a.join("a.txt"), "a").unwrap();
        git(&clone_a, &["add", "a.txt"]);
        git(&clone_a, &["commit", "-m", "c1"]);
        git(&clone_a, &["push", "-u", "origin", "main"]);
        git(&clone_a, &["checkout", "-b", "feature"]);
        fs::write(clone_a.join("f.txt"), "f1").unwrap();
        git(&clone_a, &["add", "f.txt"]);
        git(&clone_a, &["commit", "-m", "f1"]);
        git(&clone_a, &["push", "-u", "origin", "feature"]);
        (origin, clone_a)
    }

    #[test]
    fn pull_branch_fast_forwards_non_current_branch() {
        let (origin, clone_a) = setup_origin_with_feature("pullbr-ff");
        let clone_b = clone_with_config("pullbr-ff-b", &origin);
        git(&clone_b, &["branch", "--track", "feature", "origin/feature"]);

        // clone_a 推进 feature 并推送,clone_b 的 feature 落后
        fs::write(clone_a.join("f.txt"), "f2").unwrap();
        git(&clone_a, &["commit", "-am", "f2"]);
        git(&clone_a, &["push"]);

        let result = pull_branch_blocking(clone_b.to_str().unwrap(), "feature").unwrap();
        assert!(result.conflicts.is_empty());
        // 工作区仍停留在 main,本地 feature 已快进到 origin/feature
        assert_eq!(result.status.branch.as_deref(), Some("main"));
        assert_eq!(rev_parse(&clone_b, "feature"), rev_parse(&clone_b, "origin/feature"));

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn pull_branch_diverged_returns_error() {
        let (origin, clone_a) = setup_origin_with_feature("pullbr-div");
        let clone_b = clone_with_config("pullbr-div-b", &origin);
        // clone_b 在 feature 上产生本地提交后切回 main
        git(&clone_b, &["checkout", "feature"]);
        fs::write(clone_b.join("b.txt"), "b").unwrap();
        git(&clone_b, &["add", "b.txt"]);
        git(&clone_b, &["commit", "-m", "b1"]);
        git(&clone_b, &["checkout", "main"]);
        // clone_a 推进 feature,形成分叉
        fs::write(clone_a.join("f.txt"), "f2").unwrap();
        git(&clone_a, &["commit", "-am", "f2"]);
        git(&clone_a, &["push"]);

        assert!(pull_branch_blocking(clone_b.to_str().unwrap(), "feature").is_err());
        // 失败后本地 feature 未被改写
        assert_ne!(
            rev_parse(&clone_b, "feature"),
            rev_parse(&clone_b, "origin/feature")
        );

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn pull_branch_remote_deleted_returns_gone_error() {
        let (origin, clone_a) = setup_origin_with_feature("pullbr-gone");
        let clone_b = clone_with_config("pullbr-gone-b", &origin);
        git(&clone_b, &["branch", "--track", "feature", "origin/feature"]);
        // 远端删除 feature
        git(&clone_a, &["push", "origin", "--delete", "feature"]);

        let err = pull_branch_blocking(clone_b.to_str().unwrap(), "feature").unwrap_err();
        assert!(
            err.is_code(crate::error::ErrorCode::GitRemoteBranchGone),
            "实际输出: {err}"
        );

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn pull_current_branch_remote_deleted_returns_gone_error() {
        let (origin, clone_a) = setup_origin_with_feature("pull-gone");
        let clone_b = clone_with_config("pull-gone-b", &origin);
        git(&clone_b, &["checkout", "feature"]);
        // 远端删除 feature(不带 --prune,本地仍保留 origin/feature 引用)
        git(&clone_a, &["push", "origin", "--delete", "feature"]);

        let err = pull_blocking(clone_b.to_str().unwrap()).unwrap_err();
        assert!(
            err.is_code(crate::error::ErrorCode::GitRemoteBranchGone),
            "实际输出: {err}"
        );

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn push_branch_pushes_to_upstream() {
        let (origin, clone_a) = setup_origin_with_feature("pushbr-up");
        let clone_b = clone_with_config("pushbr-up-b", &origin);
        git(&clone_b, &["checkout", "feature"]);
        fs::write(clone_b.join("b.txt"), "b").unwrap();
        git(&clone_b, &["add", "b.txt"]);
        git(&clone_b, &["commit", "-m", "b1"]);
        git(&clone_b, &["checkout", "main"]);

        push_branch_blocking(clone_b.to_str().unwrap(), "feature").unwrap();
        assert_eq!(
            rev_parse(&clone_b, "feature"),
            rev_parse(&clone_b, "origin/feature")
        );

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn push_branch_without_upstream_sets_tracking() {
        let (origin, clone_a) = setup_origin_with_feature("pushbr-new");
        let clone_b = clone_with_config("pushbr-new-b", &origin);
        git(&clone_b, &["branch", "topic"]);

        push_branch_blocking(clone_b.to_str().unwrap(), "topic").unwrap();
        let out = git_command(clone_b.to_str().unwrap())
            .args(["rev-parse", "--abbrev-ref", "topic@{upstream}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "origin/topic");
        assert_eq!(rev_parse(&clone_b, "topic"), rev_parse(&clone_b, "origin/topic"));

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn branch_delete_merged_branch() {
        let dir = temp_dir("brdel-merged");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "c1"]);
        // topic 基于 main,无额外提交:视为已合并可安全删除
        git(&dir, &["branch", "topic"]);

        branch_delete_blocking(dir.to_str().unwrap(), "topic", false).unwrap();
        let out = git_command(dir.to_str().unwrap())
            .args(["branch", "--list", "topic"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn branch_delete_unmerged_requires_force() {
        let dir = temp_dir("brdel-unmerged");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "c1"]);
        // topic 有未合并进 main 的提交
        git(&dir, &["checkout", "-b", "topic"]);
        fs::write(dir.join("t.txt"), "t").unwrap();
        git(&dir, &["add", "t.txt"]);
        git(&dir, &["commit", "-m", "t1"]);
        git(&dir, &["checkout", "main"]);

        let err = branch_delete_blocking(dir.to_str().unwrap(), "topic", false).unwrap_err();
        assert!(err.is_code(ErrorCode::GitBranchNotMerged));
        // 强删成功
        branch_delete_blocking(dir.to_str().unwrap(), "topic", true).unwrap();
        let out = git_command(dir.to_str().unwrap())
            .args(["branch", "--list", "topic"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn branch_delete_rejects_current_and_empty() {
        let dir = temp_dir("brdel-current");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "c1"]);

        // 空分支名
        let err = branch_delete_blocking(dir.to_str().unwrap(), "  ", false).unwrap_err();
        assert!(err.is_code(ErrorCode::GitBranchNameRequired));
        // 当前检出分支不可删除(git 拒绝),且分支仍在(--list 输出带 * 前缀)
        assert!(branch_delete_blocking(dir.to_str().unwrap(), "main", true).is_err());
        let out = git_command(dir.to_str().unwrap())
            .args(["branch", "--list", "main"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "* main");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_branch_delete_removes_remote_ref() {
        let (origin, clone_a) = setup_origin_with_feature("rdel");
        let clone_b = clone_with_config("rdel-b", &origin);

        // 删除 origin/feature(短名含多级目录时同样按首个 '/' 拆分)
        remote_branch_delete_blocking(clone_b.to_str().unwrap(), "origin/feature").unwrap();
        let out = git_command(clone_b.to_str().unwrap())
            .args(["ls-remote", "--heads", "origin", "feature"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
        // main 不受影响
        assert_eq!(
            rev_parse(&clone_b, "origin/main"),
            rev_parse(&clone_b, "main")
        );

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn remote_branch_delete_rejects_name_without_remote() {
        let dir = temp_dir("rdel-invalid");
        init_repo(&dir);

        // 无 '/' 或段为空时无法判定远端,报 git_branch_name_required
        for name in ["main", "", "/main", "origin/"] {
            let err = remote_branch_delete_blocking(dir.to_str().unwrap(), name).unwrap_err();
            assert!(
                err.is_code(ErrorCode::GitBranchNameRequired),
                "输入 {name:?} 实际输出: {err}"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_upstream_track_formats() {
        assert_eq!(parse_upstream_track("[ahead 2, behind 3]"), (2, 3));
        assert_eq!(parse_upstream_track("[ahead 1]"), (1, 0));
        assert_eq!(parse_upstream_track("[behind 5]"), (0, 5));
        assert_eq!(parse_upstream_track("[gone]"), (0, 0));
        assert_eq!(parse_upstream_track(""), (0, 0));
    }

    #[test]
    fn list_branches_reports_upstream_tracking() {
        let origin = temp_dir("track-origin");
        git(&origin, &["init", "--bare", "-b", "main"]);

        let clone_a = temp_dir("track-a");
        git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_a, &["config", "user.email", "test@example.com"]);
        git(&clone_a, &["config", "user.name", "test"]);
        fs::write(clone_a.join("a.txt"), "a").unwrap();
        git(&clone_a, &["add", "a.txt"]);
        git(&clone_a, &["commit", "-m", "c1"]);
        git(&clone_a, &["push", "-u", "origin", "main"]);

        // feature 跟踪 origin/main;local-only 无 upstream
        git(&clone_a, &["branch", "--track", "feature", "origin/main"]);
        git(&clone_a, &["branch", "local-only"]);
        // main 本地多一个未推送提交
        fs::write(clone_a.join("a.txt"), "a2").unwrap();
        git(&clone_a, &["commit", "-am", "c2"]);

        // 另一 clone 推进 origin/main,使 main 分叉、feature 落后
        let clone_b = temp_dir("track-b");
        git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_b, &["config", "user.email", "test@example.com"]);
        git(&clone_b, &["config", "user.name", "test"]);
        fs::write(clone_b.join("b.txt"), "b").unwrap();
        git(&clone_b, &["add", "b.txt"]);
        git(&clone_b, &["commit", "-m", "c3"]);
        git(&clone_b, &["push"]);
        git(&clone_a, &["fetch", "origin"]);

        // aheady 基于最新 origin/main 再提交一个:只领先不落后
        git(&clone_a, &["checkout", "-b", "aheady", "origin/main"]);
        fs::write(clone_a.join("c.txt"), "c").unwrap();
        git(&clone_a, &["add", "c.txt"]);
        git(&clone_a, &["commit", "-m", "c4"]);
        git(&clone_a, &["checkout", "main"]);

        let branches = list_branches_blocking(clone_a.to_str().unwrap()).unwrap();
        let track = |name: &str| branches.tracking.iter().find(|t| t.name == name).cloned();

        let main = track("main").expect("main 应有 tracking");
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (1, 1));

        let feature = track("feature").expect("feature 应有 tracking");
        assert_eq!((feature.ahead, feature.behind), (0, 1));

        let aheady = track("aheady").expect("aheady 应有 tracking");
        assert_eq!((aheady.ahead, aheady.behind), (1, 0));

        assert!(track("local-only").is_none(), "无 upstream 的分支不收录");

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn pull_reports_conflicts() {
        let origin = temp_dir("pull-origin");
        git(&origin, &["init", "--bare", "-b", "main"]);

        let clone_a = temp_dir("pull-a");
        git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_a, &["config", "user.email", "test@example.com"]);
        git(&clone_a, &["config", "user.name", "test"]);
        fs::write(clone_a.join("a.txt"), "base\n").unwrap();
        git(&clone_a, &["add", "a.txt"]);
        git(&clone_a, &["commit", "-m", "c1"]);
        git(&clone_a, &["push", "-u", "origin", "main"]);

        let clone_b = temp_dir("pull-b");
        git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_b, &["config", "user.email", "test@example.com"]);
        git(&clone_b, &["config", "user.name", "test"]);
        // 显式指定合并策略,避免新版 git 对分叉分支拒绝 pull
        git(&clone_b, &["config", "pull.rebase", "false"]);

        // 双方改同一行 → 合并冲突
        fs::write(clone_a.join("a.txt"), "remote\n").unwrap();
        git(&clone_a, &["commit", "-am", "remote"]);
        git(&clone_a, &["push"]);

        fs::write(clone_b.join("a.txt"), "local\n").unwrap();
        git(&clone_b, &["commit", "-am", "local"]);

        let res = pull_blocking(clone_b.to_str().unwrap()).unwrap();
        assert!(res.status.is_repo);
        assert_eq!(res.conflicts, vec!["a.txt".to_string()]);
        assert_eq!(res.status.conflicted, 1);

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }

    #[test]
    fn commit_context_covers_staged_modified_untracked() {
        let dir = temp_dir("ctx");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        fs::write(dir.join("a.txt"), "changed").unwrap(); // 未暂存修改
        fs::write(dir.join("b.txt"), "b").unwrap(); // 已暂存新增
        git(&dir, &["add", "b.txt"]);
        fs::write(dir.join("c.txt"), "c").unwrap(); // 未跟踪

        let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
        assert!(ctx.stat.contains("a.txt"));
        assert!(ctx.stat.contains("b.txt"));
        assert!(ctx.diff.contains("changed"));
        assert!(!ctx.truncated);
        assert_eq!(ctx.untracked, vec!["c.txt".to_string()]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_context_falls_back_to_cached_without_head() {
        let dir = temp_dir("ctx-no-head");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);

        // 尚无提交:回退到暂存区 diff
        let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
        assert!(ctx.diff.contains("a.txt"));
        assert!(ctx.untracked.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    /// 在 dir 下创建一个带一次提交的嵌套 git 仓库
    fn init_nested_repo(dir: &PathBuf, name: &str) -> PathBuf {
        let nested = dir.join(name);
        fs::create_dir_all(&nested).unwrap();
        init_repo(&nested);
        fs::write(nested.join("n.txt"), "n").unwrap();
        git(&nested, &["add", "n.txt"]);
        git(&nested, &["commit", "-m", "nested init"]);
        nested
    }

    #[test]
    fn nested_repo_is_not_counted_as_untracked() {
        let dir = temp_dir("status-nested");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        let nested = init_nested_repo(&dir, "sub-lib");
        fs::write(nested.join("n.txt"), "changed").unwrap(); // 嵌套仓库内部改动
        fs::write(dir.join("b.txt"), "b").unwrap(); // 普通未跟踪文件

        // 嵌套仓库及其内部改动都不计入,只有 b.txt 算未跟踪
        let st = status(dir.to_str().unwrap()).unwrap();
        assert_eq!(st.untracked, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_context_excludes_nested_repo() {
        let dir = temp_dir("ctx-nested");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        let nested = init_nested_repo(&dir, "sub-lib");
        fs::write(nested.join("n.txt"), "changed").unwrap();
        fs::write(dir.join("b.txt"), "b").unwrap();

        // 外层只看到 b.txt;嵌套仓库不出现在 untracked,其内部改动不进 diff
        let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
        assert_eq!(ctx.untracked, vec!["b.txt".to_string()]);
        assert!(ctx.stat.is_empty());
        assert!(ctx.diff.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_context_includes_untracked_text_and_skips_binary() {
        let dir = temp_dir("ctx-untracked-content");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        fs::write(dir.join("new.txt"), "hello world").unwrap();
        fs::write(dir.join("bin.dat"), [0u8, 159, 146, 150]).unwrap(); // 含 NUL,视为二进制

        let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
        // 名称清单两个都在;内容清单只有文本文件
        assert!(ctx.untracked.contains(&"new.txt".to_string()));
        assert!(ctx.untracked.contains(&"bin.dat".to_string()));
        assert_eq!(ctx.untracked_files.len(), 1);
        assert_eq!(ctx.untracked_files[0].path, "new.txt");
        assert_eq!(ctx.untracked_files[0].content, "hello world");
        assert!(!ctx.untracked_files[0].truncated);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_context_excludes_lockfile_from_diff_but_keeps_stat() {
        let dir = temp_dir("ctx-lockfile");
        init_repo(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "a").unwrap();
        fs::write(dir.join("sub/pnpm-lock.yaml"), "lockfileVersion: 9").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "init"]);

        fs::write(dir.join("a.txt"), "changed").unwrap();
        fs::write(dir.join("sub/pnpm-lock.yaml"), "lockfileVersion: 10").unwrap();

        // diff 排除子目录中的锁文件(* 跨目录匹配);stat 仍保留,模型可感知"锁文件变了"
        let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
        assert!(ctx.diff.contains("changed"));
        assert!(!ctx.diff.contains("pnpm-lock"));
        assert!(ctx.stat.contains("pnpm-lock.yaml"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_context_includes_recent_commits() {
        let dir = temp_dir("ctx-recent");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "feat: init"]);
        fs::write(dir.join("a.txt"), "b").unwrap();
        git(&dir, &["commit", "-am", "fix: second"]);

        // 新提交在前,供模型对齐提交风格
        let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
        assert_eq!(
            ctx.recent_commits,
            vec!["fix: second".to_string(), "feat: init".to_string()]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_with_untracked_skips_nested_repo() {
        let dir = temp_dir("commit-nested");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "init"]);

        init_nested_repo(&dir, "sub-lib");
        fs::write(dir.join("b.txt"), "b").unwrap();

        // 勾选包含未跟踪:b.txt 被提交,嵌套仓库不被加成 embedded gitlink
        let st = commit_blocking(dir.to_str().unwrap(), "add b", true).unwrap();
        assert_eq!(st.untracked, 0);

        let out = git_command(dir.to_str().unwrap())
            .args(["ls-files"])
            .output()
            .unwrap();
        let tracked = String::from_utf8_lossy(&out.stdout);
        assert!(tracked.lines().any(|l| l == "b.txt"));
        assert!(!tracked.lines().any(|l| l.starts_with("sub-lib")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_log_parses_and_filters() {
        let dir = temp_dir("log");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "a").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-m", "feat: first"]);
        fs::write(dir.join("a.txt"), "a2").unwrap();
        git(&dir, &["commit", "-am", "fix: second"]);
        // 另一条作者的提交,验证 --author 过滤
        fs::write(dir.join("b.txt"), "b").unwrap();
        git(&dir, &["add", "b.txt"]);
        git(
            &dir,
            &[
                "-c",
                "user.name=other",
                "-c",
                "user.email=other@example.com",
                "commit",
                "-m",
                "docs: third",
            ],
        );

        let all = run_git_log(dir.to_str().unwrap(), None, None, None, None).unwrap();
        assert_eq!(all.len(), 3);
        // 时间倒序:最新在前
        assert_eq!(all[0].subject, "docs: third");
        assert_eq!(all[1].subject, "fix: second");
        assert_eq!(all[2].subject, "feat: first");
        assert_eq!(all[1].author, "test");
        assert!(!all[0].hash.is_empty());

        // author 过滤:仅含匹配作者的提交
        let mine = run_git_log(dir.to_str().unwrap(), None, None, None, Some("test")).unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|c| c.author == "test"));
        let nobody = run_git_log(
            dir.to_str().unwrap(),
            None,
            None,
            None,
            Some("no-such-author"),
        )
        .unwrap();
        assert!(nobody.is_empty());

        // max_count 截断
        let one = run_git_log(dir.to_str().unwrap(), None, None, Some(1), None).unwrap();
        assert_eq!(one.len(), 1);

        // until 远早于提交时间 → 空
        let none =
            run_git_log(dir.to_str().unwrap(), None, Some("2000-01-01"), None, None).unwrap();
        assert!(none.is_empty());

        // 非仓库 → 空数组而非报错
        let plain = temp_dir("log-plain");
        let res = run_git_log(plain.to_str().unwrap(), None, None, None, None).unwrap();
        assert!(res.is_empty());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&plain);
    }

    #[test]
    fn git_current_user_reads_config() {
        let dir = temp_dir("user");
        init_repo(&dir);
        let user = run_git_current_user(dir.to_str().unwrap()).unwrap();
        assert_eq!(user.name, "test");
        assert_eq!(user.email, "test@example.com");

        // 非仓库:不报错即可(字段取决于全局配置,内容不可断言)
        let plain = temp_dir("user-plain");
        run_git_current_user(plain.to_str().unwrap()).unwrap();

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&plain);
    }

    #[test]
    fn parse_worktree_porcelain_parses_blocks() {
        let text = "worktree D:/repo\n\
                    HEAD 1111111111111111111111111111111111111111\n\
                    branch refs/heads/main\n\
                    \n\
                    worktree D:/repo/.worktrees/feature-x\n\
                    HEAD 2222222222222222222222222222222222222222\n\
                    branch refs/heads/feature/x\n\
                    \n\
                    worktree D:/repo/.worktrees/det\n\
                    HEAD 3333333333333333333333333333333333333333\n\
                    detached\n";
        let list = parse_worktree_porcelain(text);
        assert_eq!(list.len(), 3);
        assert!(list[0].is_main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch.as_deref(), Some("feature/x"));
        assert!(!list[1].is_main);
        assert!(!list[1].detached);
        assert!(list[2].detached);
        assert_eq!(list[2].branch, None);
        assert_eq!(list[2].head, "3333333333333333333333333333333333333333");
    }

    #[test]
    fn worktree_add_and_remove_roundtrip() {
        let dir = temp_dir("worktree");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), "hello").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-m", "init"]);
        let path = dir.to_str().unwrap();

        // 初始只有主工作区
        let initial = list_worktrees_blocking(path).unwrap();
        assert_eq!(initial.len(), 1);
        assert!(initial[0].is_main);

        // 相对路径 + {branch} 占位符创建
        let added =
            worktree_add_blocking(path, ".worktrees/{branch}", "feature/x", true, None).unwrap();
        assert_eq!(added.len(), 2);
        let wt = added.iter().find(|w| !w.is_main).unwrap();
        assert_eq!(wt.branch.as_deref(), Some("feature/x"));
        assert!(wt.path.replace('\\', "/").contains(".worktrees/feature-x"));
        assert!(Path::new(&wt.path).join("a.txt").exists());

        // 分支已被 worktree 检出 → git_branch_checked_out
        let dup =
            worktree_add_blocking(path, ".worktrees/dup", "feature/x", true, None).unwrap_err();
        assert!(dup.is_code(ErrorCode::GitBranchCheckedOut));

        // 挂载已有(未检出)分支
        git(&dir, &["branch", "topic"]);
        let attached =
            worktree_add_blocking(path, ".worktrees/topic", "topic", false, None).unwrap();
        assert!(
            attached
                .iter()
                .any(|w| w.branch.as_deref() == Some("topic"))
        );

        // 挂载已被其它 worktree 检出的分支 → git_branch_checked_out
        let occupied =
            worktree_add_blocking(path, ".worktrees/topic2", "topic", false, None).unwrap_err();
        assert!(occupied.is_code(ErrorCode::GitBranchCheckedOut));

        // 主工作区不可删除
        let rm_main = worktree_remove_blocking(path, &initial[0].path, false, false).unwrap_err();
        assert!(rm_main.is_code(ErrorCode::GitCommandFailed));

        // 删除 worktree 并删分支
        let left = worktree_remove_blocking(path, &wt.path, false, true).unwrap();
        assert_eq!(left.len(), 2);
        assert!(!Path::new(&wt.path).exists());
        assert!(!local_branch_names(path)
            .unwrap()
            .iter()
            .any(|b| b == "feature/x"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_add_remote_branch_tracks_or_aligns_local() {
        // origin:clone_a 推 main 及 feature/topic/hotfix/ahead 四个远程分支(各含 v1 提交)
        let origin = temp_dir("wt-remote-origin");
        git(&origin, &["init", "--bare", "-b", "main"]);
        let clone_a = temp_dir("wt-remote-a");
        git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_a, &["config", "user.email", "test@example.com"]);
        git(&clone_a, &["config", "user.name", "test"]);
        fs::write(clone_a.join("a.txt"), "a").unwrap();
        git(&clone_a, &["add", "a.txt"]);
        git(&clone_a, &["commit", "-m", "c1"]);
        git(&clone_a, &["push", "-u", "origin", "main"]);
        for (name, file) in [
            ("feature", "f.txt"),
            ("topic", "t.txt"),
            ("hotfix", "h.txt"),
            ("ahead", "ah.txt"),
        ] {
            git(&clone_a, &["checkout", "-b", name, "main"]);
            fs::write(clone_a.join(file), "v1").unwrap();
            git(&clone_a, &["add", file]);
            git(&clone_a, &["commit", "-m", &format!("{name}1")]);
            git(&clone_a, &["push", "-u", "origin", name]);
        }

        let clone_b = temp_dir("wt-remote-b");
        git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
        git(&clone_b, &["config", "user.email", "test@example.com"]);
        git(&clone_b, &["config", "user.name", "test"]);
        let path_b = clone_b.to_str().unwrap();

        // 1. 本地无同名分支:挂载 origin/feature → 显式创建跟踪分支(而非游离 HEAD)
        let list =
            worktree_add_blocking(path_b, ".worktrees/feature", "origin/feature", false, None)
                .unwrap();
        let wt = list.iter().find(|w| !w.is_main).unwrap();
        assert_eq!(wt.branch.as_deref(), Some("feature"));
        assert!(!wt.detached);
        assert!(Path::new(&wt.path).join("f.txt").exists());
        let up = git_command(path_b)
            .args(["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&up.stdout).trim(), "origin/feature");

        // 远程引用落地名已被 worktree 检出 → git_branch_checked_out
        let occupied =
            worktree_add_blocking(path_b, ".worktrees/feature2", "origin/feature", false, None)
                .unwrap_err();
        assert!(occupied.is_code(ErrorCode::GitBranchCheckedOut));

        // 2. 本地同名分支落后于远程:先快进对齐到远程提交再挂载
        git(&clone_b, &["branch", "topic", "origin/topic"]);
        git(&clone_a, &["checkout", "topic"]);
        fs::write(clone_a.join("t2.txt"), "t2").unwrap();
        git(&clone_a, &["add", "t2.txt"]);
        git(&clone_a, &["commit", "-m", "t2"]);
        git(&clone_a, &["push", "origin", "topic"]);
        git(&clone_b, &["fetch", "origin"]);
        let list =
            worktree_add_blocking(path_b, ".worktrees/topic", "origin/topic", false, None).unwrap();
        let wt = list.iter().find(|w| w.branch.as_deref() == Some("topic")).unwrap();
        // 远程新提交在 worktree 中可见,且本地 topic 已对齐 origin/topic
        assert!(Path::new(&wt.path).join("t2.txt").exists());
        let local_rev = git_command(path_b).args(["rev-parse", "topic"]).output().unwrap();
        let remote_rev = git_command(path_b)
            .args(["rev-parse", "origin/topic"])
            .output()
            .unwrap();
        assert_eq!(local_rev.stdout, remote_rev.stdout);

        // 3. 本地同名分支与远程分叉:报 git_branch_diverged,不静默重置丢本地提交
        git(&clone_b, &["checkout", "-b", "hotfix", "origin/hotfix"]);
        fs::write(clone_b.join("local-only.txt"), "l").unwrap();
        git(&clone_b, &["add", "local-only.txt"]);
        git(&clone_b, &["commit", "-m", "local-only"]);
        git(&clone_b, &["checkout", "main"]);
        git(&clone_a, &["checkout", "hotfix"]);
        fs::write(clone_a.join("h2.txt"), "h2").unwrap();
        git(&clone_a, &["add", "h2.txt"]);
        git(&clone_a, &["commit", "-m", "h2"]);
        git(&clone_a, &["push", "origin", "hotfix"]);
        git(&clone_b, &["fetch", "origin"]);
        let err = worktree_add_blocking(path_b, ".worktrees/hotfix", "origin/hotfix", false, None)
            .unwrap_err();
        assert!(err.is_code(ErrorCode::GitBranchDiverged));

        // 4. 本地同名分支领先远程(远程是其祖先):直接挂载本地分支,保留本地提交
        git(&clone_b, &["checkout", "-b", "ahead", "origin/ahead"]);
        fs::write(clone_b.join("ahead-local.txt"), "l").unwrap();
        git(&clone_b, &["add", "ahead-local.txt"]);
        git(&clone_b, &["commit", "-m", "ahead-local"]);
        git(&clone_b, &["checkout", "main"]);
        let list =
            worktree_add_blocking(path_b, ".worktrees/ahead", "origin/ahead", false, None).unwrap();
        let wt = list.iter().find(|w| w.branch.as_deref() == Some("ahead")).unwrap();
        assert!(Path::new(&wt.path).join("ahead-local.txt").exists());

        let _ = fs::remove_dir_all(&origin);
        let _ = fs::remove_dir_all(&clone_a);
        let _ = fs::remove_dir_all(&clone_b);
    }
}
