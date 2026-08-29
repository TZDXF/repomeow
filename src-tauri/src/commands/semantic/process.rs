use std::collections::HashSet;
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

static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static SEM_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

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

fn register_pid(pid: u32) {
    pids().lock().unwrap_or_else(|e| e.into_inner()).insert(pid);
}

fn unregister_pid(pid: u32) {
    pids()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&pid);
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
) -> Result<SemOutput, CollectError> {
    let mut code = None;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    while let Some(event) = receiver.recv().await {
        match event {
            CommandEvent::Stdout(bytes) => {
                if stdout.len().saturating_add(bytes.len()) > STDOUT_LIMIT {
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
) -> AppResult<SemOutput> {
    let _permit = semaphore()
        .acquire_owned()
        .await
        .map_err(|error| AppError::coded(ErrorCode::SemanticToolFailed, error.to_string()))?;

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
    register_pid(pid);

    let result = tokio::time::timeout(SEM_TIMEOUT, collect_output(receiver)).await;
    unregister_pid(pid);
    match result {
        Err(_) => {
            let _ = child.kill();
            Err(AppError::coded(
                ErrorCode::SemanticAnalysisTimeout,
                format!("pid={pid}"),
            ))
        }
        Ok(Err(CollectError::OutputTooLarge)) => {
            let _ = child.kill();
            Err(AppError::coded(
                ErrorCode::SemanticOutputTooLarge,
                format!("limit={STDOUT_LIMIT}"),
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
        .status();
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
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
}
