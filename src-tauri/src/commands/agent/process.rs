use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::{AppError, AppResult, ErrorCode};

const STDERR_TAIL_MAX: usize = 8 * 1024;

pub(super) struct SpawnedAgent {
    pub child: async_process::Child,
    pub stdin: async_process::ChildStdin,
    pub stdout: async_process::ChildStdout,
    pub stderr: async_process::ChildStderr,
    pub pid: u32,
}

/// 自行 spawn 以持有 PID。Windows 隐藏窗口，Unix 建独立进程组。
pub(super) fn spawn_agent(program: &Path, args: &[String]) -> AppResult<SpawnedAgent> {
    let mut std_cmd = std::process::Command::new(program);
    std_cmd
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std_cmd.process_group(0);
    }
    let mut cmd = async_process::Command::from(std_cmd);
    // async-process 的 From<std Command> 不携带已配置管道标记，必须重设。
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use async_process::windows::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::coded(ErrorCode::AgentSpawnFailed, format!("{program:?}: {e}")))?;
    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::coded(ErrorCode::AgentSpawnFailed, "无法获取 stdin 管道"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::coded(ErrorCode::AgentSpawnFailed, "无法获取 stdout 管道"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::coded(ErrorCode::AgentSpawnFailed, "无法获取 stderr 管道"))?;
    Ok(SpawnedAgent {
        child,
        stdin,
        stdout,
        stderr,
        pid,
    })
}

/// 持续读取 stderr 并只保留尾部，防止管道写满阻塞子进程。
pub(super) fn capture_stderr(mut stderr: async_process::ChildStderr) -> Arc<Mutex<Vec<u8>>> {
    let tail = Arc::new(Mutex::new(Vec::new()));
    let task_tail = tail.clone();
    tauri::async_runtime::spawn(async move {
        use futures::AsyncReadExt;
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut stored = task_tail.lock().unwrap();
                    stored.extend_from_slice(&buf[..n]);
                    let len = stored.len();
                    if len > STDERR_TAIL_MAX {
                        stored.drain(..len - STDERR_TAIL_MAX);
                    }
                }
            }
        }
    });
    tail
}

/// stderr 尾部文本(取后 2KB,供错误信息附加诊断)
pub(super) fn tail_text(tail: &Arc<Mutex<Vec<u8>>>) -> String {
    let tail = tail.lock().unwrap();
    let start = tail.len().saturating_sub(2048);
    String::from_utf8_lossy(&tail[start..]).trim().to_string()
}

/// 按 PID 强杀进程树:Windows taskkill /T /F;Unix 杀独立进程组。
pub(super) fn kill_agent_pid(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let _ = cmd.output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .output();
    }
}
