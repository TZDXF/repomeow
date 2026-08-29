use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::AppHandle;
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;
use tokio::sync::Semaphore;

use crate::error::{AppError, AppResult, ErrorCode};

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
    app: &AppHandle,
    current_dir: Option<&Path>,
    args: &[String],
    policy: SemRunPolicy,
    request_id: Option<&str>,
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
    let (receiver, child) = command
        .spawn()
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolMissing, error.to_string()))?;
    let pid = child.pid();
    let guard = SemPidGuard::new(pid, request_id);

    // spawn 与登记之间可能已收到取消:立即补杀。
    if let Some(id) = request_id {
        if is_request_canceled(id) {
            let _ = child.kill();
            kill_pid(pid);
            drop(guard);
            return Err(canceled_error(id));
        }
    }

    let result = tokio::time::timeout(policy.timeout, collect_output(receiver, policy.stdout_limit)).await;
    let canceled = request_id.is_some_and(is_request_canceled);
    drop(guard);
    match result {
        Err(_) => {
            let _ = child.kill();
            kill_pid(pid);
            Err(AppError::coded(
                ErrorCode::SemanticAnalysisTimeout,
                format!("pid={pid}"),
            ))
        }
        Ok(_) if canceled => {
            let _ = child.kill();
            kill_pid(pid);
            Err(canceled_error(request_id.unwrap_or_default()))
        }
        Ok(Err(CollectError::OutputTooLarge)) => {
            let _ = child.kill();
            Err(AppError::coded(
                ErrorCode::SemanticOutputTooLarge,
                format!("limit={}", policy.stdout_limit),
            ))
        }
        Ok(Err(CollectError::Process(error))) => {
            let _ = child.kill();
            Err(AppError::coded(ErrorCode::SemanticToolFailed, error))
        }
        Ok(Ok(output)) => Ok(output),
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
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
    requests()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
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
