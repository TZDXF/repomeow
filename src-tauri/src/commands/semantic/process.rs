use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::AppHandle;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

use crate::error::{AppError, AppResult, ErrorCode};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const SEM_TIMEOUT: Duration = Duration::from_secs(60);
const STDOUT_LIMIT: usize = 32 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const MAX_CONCURRENCY: usize = 2;

/// 单个 sem 命令的运行策略:超时与 stdout 上限按命令负载分档。
#[derive(Debug, Clone, Copy)]
pub(super) struct SemRunPolicy {
    pub timeout: Duration,
    pub stdout_limit: usize,
}

impl SemRunPolicy {
    /// 默认策略(status / commit diff 等既有调用)。
    pub const DEFAULT: Self = Self {
        timeout: SEM_TIMEOUT,
        stdout_limit: STDOUT_LIMIT,
    };
    /// 轻量查询(entities / find / callers / refs)。
    pub const NAV: Self = Self {
        timeout: Duration::from_secs(60),
        stdout_limit: 8 * 1024 * 1024,
    };
    /// 重查询(impact / blame / log)。
    pub const HEAVY: Self = Self {
        timeout: Duration::from_secs(120),
        stdout_limit: 16 * 1024 * 1024,
    };
    /// context / 工作区 diff 摘要。
    pub const CONTEXT: Self = Self {
        timeout: Duration::from_secs(120),
        stdout_limit: 8 * 1024 * 1024,
    };
}

static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static SEM_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
static REQUESTS: OnceLock<Mutex<HashMap<String, RequestSlot>>> = OnceLock::new();

#[derive(Default)]
struct RequestSlot {
    canceled: bool,
    pids: HashSet<u32>,
}

pub(super) struct SemOutput {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// sem 二进制的启动方式:应用内经 Tauri sidecar 插件;headless 进程(内置 MCP
/// server 的 `--mcp` 模式)没有 AppHandle,按可执行文件旁的显式路径直接 spawn。
#[derive(Clone)]
pub(crate) enum SemLauncher {
    App(AppHandle),
    Standalone(PathBuf),
}

impl From<&AppHandle> for SemLauncher {
    fn from(app: &AppHandle) -> Self {
        Self::App(app.clone())
    }
}

/// headless 场景定位 sem sidecar:Tauri 构建/开发时会把 externalBin 复制到
/// 可执行文件旁(dev 在 target/debug/,release 在安装目录),文件名通常为
/// `sem-<target-triple>[.exe]`;按 triple 后缀优先、裸名兜底探测。
pub(crate) fn resolve_sem_binary() -> AppResult<PathBuf> {
    let missing = |detail: String| AppError::coded(ErrorCode::SemanticToolMissing, detail);
    let exe = std::env::current_exe().map_err(|error| missing(error.to_string()))?;
    let dir = exe
        .parent()
        .ok_or_else(|| missing("cannot resolve executable directory".to_string()))?;
    let mut triple_named = None;
    let entries = std::fs::read_dir(dir).map_err(|error| missing(error.to_string()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_sidecar = name.strip_prefix("sem-").is_some_and(|rest| {
            !rest.is_empty() && (!cfg!(windows) || rest.ends_with(".exe"))
        });
        if is_sidecar && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            triple_named = Some(entry.path());
            break;
        }
    }
    if let Some(path) = triple_named {
        return Ok(path);
    }
    let plain = dir.join(if cfg!(windows) { "sem.exe" } else { "sem" });
    if plain.is_file() {
        return Ok(plain);
    }
    Err(missing(format!(
        "sem sidecar not found next to {}",
        exe.display()
    )))
}

enum CollectError {
    OutputTooLarge,
    Process(String),
}

fn semaphore() -> Arc<Semaphore> {
    SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENCY)))
        .clone()
}

fn pids() -> &'static Mutex<HashSet<u32>> {
    SEM_PIDS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn requests() -> &'static Mutex<HashMap<String, RequestSlot>> {
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_pid(pid: u32) {
    pids().lock().unwrap_or_else(|e| e.into_inner()).insert(pid);
}

fn unregister_pid(pid: u32) {
    pids()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&pid);
}

fn is_request_canceled(request_id: &str) -> bool {
    requests()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(request_id)
        .is_some_and(|slot| slot.canceled)
}

/// 取消指定请求:标记取消并杀掉其当前进程;返回是否存在该请求。
pub(super) fn cancel_request(request_id: &str) -> bool {
    let targets = {
        let mut registry = requests().lock().unwrap_or_else(|e| e.into_inner());
        let Some(slot) = registry.get_mut(request_id) else {
            return false;
        };
        slot.canceled = true;
        slot.pids.iter().copied().collect::<Vec<_>>()
    };
    for pid in targets {
        kill_pid(pid);
    }
    true
}

/// RAII:注册/注销 sem 子进程 PID(全局退出清理表 + 请求取消表)。
struct SemPidGuard {
    pid: u32,
    request_id: Option<String>,
}

impl SemPidGuard {
    fn new(pid: u32, request_id: Option<&str>) -> Self {
        register_pid(pid);
        if let Some(id) = request_id {
            requests()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(id.to_string())
                .or_default()
                .pids
                .insert(pid);
        }
        Self {
            pid,
            request_id: request_id.map(str::to_string),
        }
    }
}

impl Drop for SemPidGuard {
    fn drop(&mut self) {
        unregister_pid(self.pid);
        if let Some(id) = &self.request_id {
            let mut registry = requests().lock().unwrap_or_else(|e| e.into_inner());
            let remove = if let Some(slot) = registry.get_mut(id) {
                slot.pids.remove(&self.pid);
                slot.pids.is_empty()
            } else {
                false
            };
            if remove {
                registry.remove(id);
            }
        }
    }
}

fn canceled_error(request_id: &str) -> AppError {
    AppError::coded(
        ErrorCode::SemanticCanceled,
        format!("request_id={request_id}"),
    )
}

fn append_bounded_tail(buffer: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    if bytes.len() >= limit {
        buffer.clear();
        buffer.extend_from_slice(&bytes[bytes.len() - limit..]);
    } else {
        let overflow = buffer
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(limit);
        if overflow > 0 {
            buffer.drain(..overflow);
        }
        buffer.extend_from_slice(bytes);
    }
}

async fn collect_output(
    mut receiver: tauri::async_runtime::Receiver<CommandEvent>,
    stdout_limit: usize,
) -> Result<SemOutput, CollectError> {
    let mut code = None;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    while let Some(event) = receiver.recv().await {
        match event {
            CommandEvent::Stdout(bytes) => {
                if stdout.len().saturating_add(bytes.len()) > stdout_limit {
                    return Err(CollectError::OutputTooLarge);
                }
                stdout.extend_from_slice(&bytes);
            }
            CommandEvent::Stderr(bytes) => append_bounded_tail(&mut stderr, &bytes, STDERR_LIMIT),
            CommandEvent::Terminated(payload) => code = payload.code,
            CommandEvent::Error(error) => return Err(CollectError::Process(error)),
            _ => {}
        }
    }
    Ok(SemOutput {
        code,
        stdout,
        stderr,
    })
}

pub(super) async fn run_sem(
    launcher: &SemLauncher,
    current_dir: Option<&Path>,
    args: &[String],
    policy: SemRunPolicy,
    request_id: Option<&str>,
) -> AppResult<SemOutput> {
    run_sem_inner(launcher, current_dir, args, policy, request_id, None).await
}

/// 运行需要 stdin 的 sem 命令。写完立即关闭 stdin 管道，
/// 否则 `sem diff --stdin` 等读取标准输入的命令会一直等待 EOF。
pub(super) async fn run_sem_with_input(
    launcher: &SemLauncher,
    current_dir: Option<&Path>,
    args: &[String],
    policy: SemRunPolicy,
    request_id: Option<&str>,
    input: &[u8],
) -> AppResult<SemOutput> {
    run_sem_inner(launcher, current_dir, args, policy, request_id, Some(input)).await
}

/// 已 spawn 的 sem 子进程:Tauri sidecar(CommandEvent 流)或独立 tokio 进程。
/// pid 在 spawn 时固定,stdin 写完即关(sidecar 靠丢弃 CommandChild)。
enum SemChild {
    Shell {
        pid: u32,
        child: Option<CommandChild>,
        receiver: Option<tauri::async_runtime::Receiver<CommandEvent>>,
    },
    Std {
        pid: u32,
        child: tokio::process::Child,
    },
}

impl SemChild {
    fn pid(&self) -> u32 {
        match self {
            Self::Shell { pid, .. } | Self::Std { pid, .. } => *pid,
        }
    }

    async fn write_stdin(&mut self, bytes: &[u8]) -> Result<(), String> {
        match self {
            Self::Shell { child, .. } => {
                let result = child.as_mut().expect("spawned child missing").write(bytes);
                // CommandChild 持有 stdin_writer；drop 后 sem 才能读到 EOF。
                drop(child.take());
                result.map_err(|error| format!("stdin write failed: {error}"))
            }
            Self::Std { child, .. } => {
                let mut stdin = child.stdin.take().expect("stdin piped");
                let result = stdin
                    .write_all(bytes)
                    .await
                    .map_err(|error| format!("stdin write failed: {error}"));
                drop(stdin);
                result
            }
        }
    }

    fn terminate(&mut self) {
        match self {
            Self::Shell { pid, child, .. } => {
                if let Some(child) = child.take() {
                    let _ = child.kill();
                }
                kill_pid(*pid);
            }
            Self::Std { pid, child } => {
                let _ = child.start_kill();
                kill_pid(*pid);
            }
        }
    }

    async fn collect(&mut self, stdout_limit: usize) -> Result<SemOutput, CollectError> {
        match self {
            Self::Shell { receiver, .. } => {
                collect_output(receiver.take().expect("receiver missing"), stdout_limit).await
            }
            Self::Std { child, .. } => collect_std_output(child, stdout_limit).await,
        }
    }
}

fn spawn_sidecar(
    app: &AppHandle,
    current_dir: Option<&Path>,
    args: &[String],
) -> AppResult<SemChild> {
    let mut command = app
        .shell()
        .sidecar("sem")
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolMissing, error.to_string()))?
        .args(args)
        .env("DO_NOT_TRACK", "1")
        .env("SEM_NO_TELEMETRY", "1")
        .env("SEM_NO_UPDATE_CHECK", "1")
        .env("SEM_NO_NETWORK", "1")
        .env("NO_COLOR", "1")
        .set_raw_out(true);
    if let Some(path) = current_dir {
        command = command.current_dir(path);
    }
    let (receiver, spawned) = command
        .spawn()
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolMissing, error.to_string()))?;
    Ok(SemChild::Shell {
        pid: spawned.pid(),
        child: Some(spawned),
        receiver: Some(receiver),
    })
}

fn spawn_standalone(
    bin: &Path,
    current_dir: Option<&Path>,
    args: &[String],
) -> AppResult<SemChild> {
    let mut command = tokio::process::Command::new(bin);
    command
        .args(args)
        .env("DO_NOT_TRACK", "1")
        .env("SEM_NO_TELEMETRY", "1")
        .env("SEM_NO_UPDATE_CHECK", "1")
        .env("SEM_NO_NETWORK", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    if let Some(path) = current_dir {
        command.current_dir(path);
    }
    let child = command
        .spawn()
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolMissing, error.to_string()))?;
    let pid = child.id().unwrap_or(0);
    Ok(SemChild::Std { pid, child })
}

/// 独立进程(tokio)的输出采集:与 collect_output 同一语义——stdout 超限即失败,
/// stderr 只保留尾部,进程退出码取 wait 结果。
async fn collect_std_output(
    child: &mut tokio::process::Child,
    stdout_limit: usize,
) -> Result<SemOutput, CollectError> {
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let read_out = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = stdout_pipe
                .read(&mut buf)
                .await
                .map_err(|error| CollectError::Process(error.to_string()))?;
            if n == 0 {
                break;
            }
            if stdout.len().saturating_add(n) > stdout_limit {
                return Err(CollectError::OutputTooLarge);
            }
            stdout.extend_from_slice(&buf[..n]);
        }
        Ok(())
    };
    let read_err = async {
        let mut buf = [0u8; 8192];
        loop {
            let n = stderr_pipe
                .read(&mut buf)
                .await
                .map_err(|error| CollectError::Process(error.to_string()))?;
            if n == 0 {
                break;
            }
            append_bounded_tail(&mut stderr, &buf[..n], STDERR_LIMIT);
        }
        Ok(())
    };
    let (out_result, err_result, status) = tokio::join!(read_out, read_err, child.wait());
    out_result?;
    err_result?;
    let status = status.map_err(|error| CollectError::Process(error.to_string()))?;
    Ok(SemOutput {
        code: status.code(),
        stdout,
        stderr,
    })
}

async fn run_sem_inner(
    launcher: &SemLauncher,
    current_dir: Option<&Path>,
    args: &[String],
    policy: SemRunPolicy,
    request_id: Option<&str>,
    input: Option<&[u8]>,
) -> AppResult<SemOutput> {
    if let Some(id) = request_id {
        // 请求槽在 SemPidGuard 中按 PID 登记,这里仅保证槽位存在,便于等待
        // 信号量期间也能被 semantic_cancel 标记。
        requests()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_string())
            .or_default();
    }

    let _permit = semaphore()
        .acquire_owned()
        .await
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolFailed, error.to_string()))?;

    if let Some(id) = request_id {
        if is_request_canceled(id) {
            requests()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(id);
            return Err(canceled_error(id));
        }
    }

    let mut spawned = match launcher {
        SemLauncher::App(app) => spawn_sidecar(app, current_dir, args)?,
        SemLauncher::Standalone(bin) => spawn_standalone(bin, current_dir, args)?,
    };
    let pid = spawned.pid();
    let guard = SemPidGuard::new(pid, request_id);

    // spawn 与登记之间可能已收到取消:立即补杀。
    if let Some(id) = request_id {
        if is_request_canceled(id) {
            spawned.terminate();
            drop(guard);
            return Err(canceled_error(id));
        }
    }

    if let Some(bytes) = input {
        if let Err(error) = spawned.write_stdin(bytes).await {
            spawned.terminate();
            drop(guard);
            return Err(AppError::coded(ErrorCode::SemanticToolFailed, error));
        }
    }

    let result = tokio::time::timeout(policy.timeout, spawned.collect(policy.stdout_limit)).await;
    let canceled = request_id.is_some_and(is_request_canceled);
    drop(guard);
    match result {
        Err(_) => {
            spawned.terminate();
            Err(AppError::coded(
                ErrorCode::SemanticAnalysisTimeout,
                format!("pid={pid}"),
            ))
        }
        Ok(_) if canceled => {
            spawned.terminate();
            Err(canceled_error(request_id.unwrap_or_default()))
        }
        Ok(Err(CollectError::OutputTooLarge)) => {
            spawned.terminate();
            Err(AppError::coded(
                ErrorCode::SemanticOutputTooLarge,
                format!("limit={}", policy.stdout_limit),
            ))
        }
        Ok(Err(CollectError::Process(error))) => {
            spawned.terminate();
            Err(AppError::coded(ErrorCode::SemanticToolFailed, error))
        }
        Ok(Ok(output)) => Ok(output),
    }
}
#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

pub fn cleanup_on_exit() {
    let all: Vec<u32> = pids()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .collect();
    for pid in all {
        kill_pid(pid);
    }
    requests().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_buffer_keeps_the_latest_bytes() {
        let mut buffer = b"1234".to_vec();
        append_bounded_tail(&mut buffer, b"5678", 6);
        assert_eq!(buffer, b"345678");
        append_bounded_tail(&mut buffer, b"abcdefgh", 6);
        assert_eq!(buffer, b"cdefgh");
    }

    #[test]
    fn pid_guard_registers_and_unregisters_without_leak() {
        let pid = 424_242;
        {
            let _guard = SemPidGuard::new(pid, Some("req-1"));
            assert!(pids().lock().unwrap().contains(&pid));
            let registry = requests().lock().unwrap();
            assert!(registry.get("req-1").is_some_and(|s| s.pids.contains(&pid)));
        }
        assert!(!pids().lock().unwrap().contains(&pid));
        assert!(!requests().lock().unwrap().contains_key("req-1"));
    }

    #[test]
    fn cancel_unknown_request_returns_false() {
        assert!(!cancel_request("no-such-request"));
    }

    #[test]
    fn cancel_marks_slot_canceled() {
        let pid = 313_371;
        {
            let _guard = SemPidGuard::new(pid, Some("req-2"));
            assert!(cancel_request("req-2"));
            assert!(is_request_canceled("req-2"));
        }
        assert!(!requests().lock().unwrap().contains_key("req-2"));
    }
}
