//! 执行环境(tokio 后端):对齐 `packages/agent/src/harness/env/nodejs.ts`。
//!
//! `TokioEnv` 用 `tokio::fs` 实现 [`FileSystem`]、`tokio::process` 实现
//! [`Shell`];shell 解析与蓝本一致 —— 优先 Git Bash(Windows 下查找
//! ProgramFiles/PATH,老式 WSL bash 走 stdin 传输),非 Windows 依次
//! `/bin/bash` → `which bash` → `sh -c`。超时/中止经 kill 进程树收尾。
//!
//! 偏差:file:// URL 展开(桌面端无需)与 Node `readline` 的流式逐行读取
//! (readTextLines 一次性读取后裁剪)未复刻;unpaired surrogate 清洗不存在于
//! Rust UTF-8 语义。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::agent::harness::types::{
    err, ok, CreateDirOptions, CreateTempFileOptions, ExecOutcome, ExecutionError,
    ExecutionErrorCode, FileContent, FileError, FileErrorCode, FileInfo, FileKind, FileSystem,
    ReadTextLinesOptions, RemoveOptions, Result, Shell, ShellExecOptions,
};
use crate::agent::types::AbortSignal;
use crate::agent::harness::uuid::uuid_v7;

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;
const EXIT_STDIO_GRACE_MS: u64 = 100;

fn resolve_timeout_seconds(timeout: Option<f64>) -> Result<Option<f64>, ExecutionError> {
    let Some(timeout) = timeout else {
        return ok(None);
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return err(ExecutionError::new(
            ExecutionErrorCode::Timeout,
            "Invalid timeout: must be a finite number of seconds",
        ));
    }
    if timeout > MAX_TIMEOUT_SECONDS {
        return err(ExecutionError::new(
            ExecutionErrorCode::Timeout,
            format!("Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"),
        ));
    }
    ok(Some(timeout))
}

/// 路径解析:支持 `~`/`~/` 展开(对齐 TS `resolvePath`)。
fn resolve_path(cwd: &str, path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = home_dir() {
            return normalize_absolute(&home);
        }
    } else if path.starts_with("~/") || (cfg!(windows) && path.starts_with("~\\")) {
        if let Some(home) = home_dir() {
            return normalize_absolute(&home.join(&path[2..]));
        }
    }
    let candidate = Path::new(path);
    let candidate = if candidate.is_absolute() {
        PathBuf::from(candidate)
    } else {
        Path::new(cwd).join(candidate)
    };
    normalize_absolute(&candidate)
}

/// 语法归一化:词法展开 `.`/`..`(与 Node path.resolve 一致,不触盘)。
fn normalize_absolute(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let mut prefix: Option<String> = None;
    let mut body_start = 0usize;
    if cfg!(windows) && text.len() >= 2 && text.as_bytes()[1] == b':' {
        prefix = Some(text[..2].to_string());
        body_start = 2;
    }

    let mut stack: Vec<String> = prefix.iter().cloned().collect();
    for component in text[body_start..].split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                if stack.len() > prefix.as_ref().map_or(0, |_| 1) {
                    stack.pop();
                }
            }
            other => stack.push(other.to_string()),
        }
    }

    let mut result = PathBuf::new();
    if prefix.is_some() {
        for part in &stack {
            result.push(part);
        }
    } else {
        result.push("/");
        for part in stack {
            result.push(part);
        }
    }
    result
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn file_info_from_metadata(path: &str, metadata: &std::fs::Metadata) -> Result<FileInfo, FileError> {
    let kind = if metadata.is_symlink() {
        FileKind::Symlink
    } else if metadata.is_file() {
        FileKind::File
    } else if metadata.is_dir() {
        FileKind::Directory
    } else {
        return err(FileError::new(FileErrorCode::Invalid, "Unsupported file type").with_path(path));
    };
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as f64)
        .unwrap_or(0.0);
    let name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    ok(FileInfo {
        name,
        path: path.to_string(),
        kind,
        size: metadata.len(),
        mtime_ms,
    })
}

fn to_file_error(error: io::Error, fallback_path: Option<&str>) -> FileError {
    let message = error.to_string();
    let code = match error.kind() {
        io::ErrorKind::NotFound => FileErrorCode::NotFound,
        io::ErrorKind::PermissionDenied => FileErrorCode::PermissionDenied,
        io::ErrorKind::NotADirectory => FileErrorCode::NotDirectory,
        io::ErrorKind::IsADirectory => FileErrorCode::IsDirectory,
        io::ErrorKind::InvalidInput => FileErrorCode::Invalid,
        _ => match error.raw_os_error() {
            // Windows ERROR_FILE_NOT_FOUND(2)/ ERROR_PATH_NOT_FOUND(3)。
            Some(3) => FileErrorCode::NotFound,
            // Windows ERROR_ACCESS_DENIED。
            Some(5) => FileErrorCode::PermissionDenied,
            // unix ENOENT(2)。
            Some(2) => FileErrorCode::NotFound,
            // unix EPERM(1)/ EACCES(13)。
            Some(1) | Some(13) => FileErrorCode::PermissionDenied,
            // ENOTDIR。
            Some(20) => FileErrorCode::NotDirectory,
            // EISDIR。
            Some(21) => FileErrorCode::IsDirectory,
            // EINVAL。
            Some(22) => FileErrorCode::Invalid,
            _ => FileErrorCode::Unknown,
        },
    };
    let mut file_error = FileError::new(code, message);
    if let Some(path) = fallback_path {
        file_error = file_error.with_path(path);
    }
    file_error
}

async fn path_exists(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

struct ShellConfig {
    shell: String,
    args: Vec<String>,
    /// stdin 传输(老式 WSL bash)。
    command_transport_stdin: bool,
}

fn is_legacy_wsl_bash_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_lowercase();
    normalized.starts_with("c:\\windows\\system32\\bash.exe")
        || normalized.starts_with("c:\\windows\\sysnative\\bash.exe")
}

fn get_bash_shell_config(shell: &str) -> ShellConfig {
    if is_legacy_wsl_bash_path(shell) {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-s".to_string()],
            command_transport_stdin: true,
        }
    } else {
        ShellConfig {
            shell: shell.to_string(),
            args: vec!["-c".to_string()],
            command_transport_stdin: false,
        }
    }
}

async fn find_bash_on_path() -> Option<String> {
    let (program, arg) = if cfg!(windows) {
        ("where", "bash.exe")
    } else {
        ("which", "bash")
    };
    let output = tokio::process::Command::new(program)
        .arg(arg)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_match = stdout.trim().lines().next()?.trim().to_string();
    if first_match.is_empty() {
        return None;
    }
    if path_exists(Path::new(&first_match)).await {
        Some(first_match)
    } else {
        None
    }
}

async fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, ExecutionError> {
    if let Some(custom_shell_path) = custom_shell_path {
        if path_exists(Path::new(custom_shell_path)).await {
            return ok(get_bash_shell_config(custom_shell_path));
        }
        return err(ExecutionError::new(
            ExecutionErrorCode::ShellUnavailable,
            format!("Custom shell path not found: {custom_shell_path}"),
        ));
    }
    if cfg!(windows) {
        let mut candidates: Vec<String> = Vec::new();
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(
                PathBuf::from(program_files)
                    .join("Git")
                    .join("bin")
                    .join("bash.exe")
                    .to_string_lossy()
                    .to_string(),
            );
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            candidates.push(
                PathBuf::from(program_files_x86)
                    .join("Git")
                    .join("bin")
                    .join("bash.exe")
                    .to_string_lossy()
                    .to_string(),
            );
        }
        for candidate in &candidates {
            if path_exists(Path::new(candidate)).await {
                return ok(get_bash_shell_config(candidate));
            }
        }
        if let Some(bash_on_path) = find_bash_on_path().await {
            return ok(get_bash_shell_config(&bash_on_path));
        }
        return err(ExecutionError::new(
            ExecutionErrorCode::ShellUnavailable,
            "No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Configure an explicit shellPath",
        ));
    }

    if path_exists(Path::new("/bin/bash")).await {
        return ok(get_bash_shell_config("/bin/bash"));
    }
    if let Some(bash_on_path) = find_bash_on_path().await {
        return ok(get_bash_shell_config(&bash_on_path));
    }
    ok(ShellConfig {
        shell: "sh".to_string(),
        args: vec!["-c".to_string()],
        command_transport_stdin: false,
    })
}

/// 终止进程树:Windows 用 taskkill /F /T;unix 先杀进程组再杀单进程。
fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let taskkill = system_root.join("System32").join("taskkill.exe");
        let _ = std::process::Command::new(taskkill)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(unix)]
    {
        mod libc_kill {
            // unix 目标自动链接 libc;显式 extern 避免额外依赖声明。
            extern "C" {
                pub fn kill(pid: i32, sig: i32) -> i32;
            }
        }
        let group_result = unsafe { libc_kill::kill(-(pid as i32), 9) };
        if group_result != 0 {
            let _ = unsafe { libc_kill::kill(pid as i32, 9) };
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

/// tokio 执行环境:文件系统 + shell(对齐 TS `NodeExecutionEnv`)。
pub struct TokioEnv {
    cwd: PathBuf,
    /// cwd 的字符串缓存(cwd() 返回 &str 需要)。
    cwd_text: String,
    shell_path: Option<String>,
    shell_env: Option<HashMap<String, String>>,
    active_child_pids: Arc<Mutex<std::collections::HashSet<u32>>>,
    temp_counter: AtomicU64,
}

/// TokioEnv 构造选项(对齐 NodeExecutionEnv 构造参数)。
pub struct TokioEnvOptions {
    pub cwd: PathBuf,
    pub shell_path: Option<String>,
    pub shell_env: Option<HashMap<String, String>>,
}

impl TokioEnv {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self::with_options(TokioEnvOptions {
            cwd: cwd.into(),
            shell_path: None,
            shell_env: None,
        })
    }

    pub fn with_options(options: TokioEnvOptions) -> Self {
        let normalized = normalize_absolute(&options.cwd);
        let cwd_text = normalized.to_string_lossy().to_string();
        Self {
            cwd: normalized,
            cwd_text,
            shell_path: options.shell_path,
            shell_env: options.shell_env,
            active_child_pids: Arc::new(Mutex::new(std::collections::HashSet::new())),
            temp_counter: AtomicU64::new(0),
        }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        resolve_path(&self.cwd_text, path)
    }
}

impl FileSystem for TokioEnv {
    fn cwd(&self) -> &str {
        &self.cwd_text
    }

    fn absolute_path<'a>(
        &'a self,
        path: String,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move { ok(self.resolve(&path).to_string_lossy().to_string()) })
    }

    fn join_path<'a>(
        &'a self,
        parts: Vec<String>,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let mut joined = PathBuf::new();
            for part in parts {
                // 与 Node path.join 一致:逐段拼接。
                joined.push(part);
            }
            ok(joined.to_string_lossy().to_string())
        })
    }

    fn read_text_file<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            match tokio::fs::read(&resolved).await {
                Ok(bytes) => ok(String::from_utf8_lossy(&bytes).to_string()),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn read_text_lines<'a>(
        &'a self,
        path: String,
        options: ReadTextLinesOptions,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &options.abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            if options.max_lines == Some(0) {
                return ok(Vec::new());
            }
            match tokio::fs::read(&resolved).await {
                Ok(bytes) => {
                    let content = String::from_utf8_lossy(&bytes);
                    let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
                    if content.ends_with('\n') {
                        lines.pop();
                    }
                    // 归一化 CRLF(对齐 readline crlfDelay)。
                    for line in &mut lines {
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    if let Some(max_lines) = options.max_lines {
                        lines.truncate(max_lines);
                    }
                    ok(lines)
                }
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn read_binary_file<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            match tokio::fs::read(&resolved).await {
                Ok(bytes) => ok(bytes),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn write_file<'a>(
        &'a self,
        path: String,
        content: FileContent,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            if let Some(parent) = resolved.parent() {
                if let Err(error) = tokio::fs::create_dir_all(parent).await {
                    return err(to_file_error(error, Some(&path_text)));
                }
            }
            if let Some(signal) = &abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            let bytes = match content {
                FileContent::Text(text) => text.into_bytes(),
                FileContent::Binary(bytes) => bytes,
            };
            match tokio::fs::write(&resolved, bytes).await {
                Ok(()) => ok(()),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn append_file<'a>(
        &'a self,
        path: String,
        content: FileContent,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(parent) = resolved.parent() {
                if let Err(error) = tokio::fs::create_dir_all(parent).await {
                    return err(to_file_error(error, Some(&path_text)));
                }
            }
            let bytes = match content {
                FileContent::Text(text) => text.into_bytes(),
                FileContent::Binary(bytes) => bytes,
            };
            let mut file = match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&resolved)
                .await
            {
                Ok(file) => file,
                Err(error) => return err(to_file_error(error, Some(&path_text))),
            };
            use tokio::io::AsyncWriteExt;
            match file.write_all(&bytes).await {
                Ok(()) => ok(()),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn rename_file<'a>(
        &'a self,
        source_path: String,
        destination_path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let source = self.resolve(&source_path);
            let destination = self.resolve(&destination_path);
            let destination_text = destination.to_string_lossy().to_string();
            if let Some(signal) = &abort_signal {
                if signal.is_cancelled() {
                    return err(
                        FileError::new(FileErrorCode::Aborted, "aborted").with_path(&destination_text),
                    );
                }
            }
            match tokio::fs::rename(&source, &destination).await {
                Ok(()) => ok(()),
                Err(error) => err(to_file_error(
                    error,
                    Some(&source.to_string_lossy().to_string()),
                )),
            }
        })
    }

    fn file_info<'a>(&'a self, path: String) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            match tokio::fs::symlink_metadata(&resolved).await {
                Ok(metadata) => file_info_from_metadata(&path_text, &metadata),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn list_dir<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            let mut entries = match tokio::fs::read_dir(&resolved).await {
                Ok(entries) => entries,
                Err(error) => return err(to_file_error(error, Some(&path_text))),
            };
            let mut infos: Vec<FileInfo> = Vec::new();
            loop {
                match entries.next_entry().await {
                    Ok(Some(entry)) => {
                        if let Some(signal) = &abort_signal {
                            if signal.is_cancelled() {
                                return err(
                                    FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text),
                                );
                            }
                        }
                        let entry_path = entry.path();
                        let entry_text = entry_path.to_string_lossy().to_string();
                        match tokio::fs::symlink_metadata(&entry_path).await {
                            Ok(metadata) => {
                                if let Ok(info) = file_info_from_metadata(&entry_text, &metadata) {
                                    infos.push(info);
                                }
                            }
                            Err(error) => return err(to_file_error(error, Some(&entry_text))),
                        }
                    }
                    Ok(None) => break,
                    Err(error) => return err(to_file_error(error, Some(&path_text))),
                }
            }
            ok(infos)
        })
    }

    fn canonical_path<'a>(
        &'a self,
        path: String,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            match tokio::fs::canonicalize(&resolved).await {
                Ok(canonical) => ok(canonical.to_string_lossy().to_string()),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn exists<'a>(
        &'a self,
        path: String,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<bool, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            match tokio::fs::symlink_metadata(&resolved).await {
                Ok(_) => ok(true),
                Err(error) if error.kind() == io::ErrorKind::NotFound => ok(false),
                Err(error) => err(to_file_error(
                    error,
                    Some(&resolved.to_string_lossy().to_string()),
                )),
            }
        })
    }

    fn create_dir<'a>(
        &'a self,
        path: String,
        options: CreateDirOptions,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &options.abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            let recursive = options.recursive.unwrap_or(true);
            let result = if recursive {
                tokio::fs::create_dir_all(&resolved).await
            } else {
                tokio::fs::create_dir(&resolved).await
            };
            match result {
                Ok(()) => ok(()),
                Err(error) => err(to_file_error(error, Some(&path_text))),
            }
        })
    }

    fn remove<'a>(
        &'a self,
        path: String,
        options: RemoveOptions,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(&path);
            let path_text = resolved.to_string_lossy().to_string();
            if let Some(signal) = &options.abort_signal {
                if signal.is_cancelled() {
                    return err(FileError::new(FileErrorCode::Aborted, "aborted").with_path(&path_text));
                }
            }
            let recursive = options.recursive.unwrap_or(false);
            let force = options.force.unwrap_or(false);
            let metadata = tokio::fs::symlink_metadata(&resolved).await;
            match metadata {
                Err(error) => {
                    if force && error.kind() == io::ErrorKind::NotFound {
                        ok(())
                    } else {
                        err(to_file_error(error, Some(&path_text)))
                    }
                }
                Ok(metadata) => {
                    let result = if metadata.is_dir() && !metadata.is_symlink() {
                        if recursive {
                            tokio::fs::remove_dir_all(&resolved).await
                        } else {
                            tokio::fs::remove_dir(&resolved).await
                        }
                    } else {
                        tokio::fs::remove_file(&resolved).await
                    };
                    match result {
                        Ok(()) => ok(()),
                        Err(error) => {
                            if force && error.kind() == io::ErrorKind::NotFound {
                                ok(())
                            } else {
                                err(to_file_error(error, Some(&path_text)))
                            }
                        }
                    }
                }
            }
        })
    }

    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<String>,
        _abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let prefix = prefix.unwrap_or_else(|| "tmp-".to_string());
            let unique = format!(
                "{}{}{}",
                prefix,
                self.temp_counter.fetch_add(1, Ordering::SeqCst),
                uuid_v7().replace('-', "")
            );
            let dir = std::env::temp_dir().join(unique);
            match tokio::fs::create_dir_all(&dir).await {
                Ok(()) => ok(dir.to_string_lossy().to_string()),
                Err(error) => err(to_file_error(error, None)),
            }
        })
    }

    fn create_temp_file<'a>(
        &'a self,
        options: CreateTempFileOptions,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let dir = self.create_temp_dir(Some("tmp-".to_string()), None).await?;
            let file_path = Path::new(&dir).join(format!(
                "{}{}{}",
                options.prefix.unwrap_or_default(),
                uuid_v7().replace('-', ""),
                options.suffix.unwrap_or_default()
            ));
            match tokio::fs::write(&file_path, b"").await {
                Ok(()) => ok(file_path.to_string_lossy().to_string()),
                Err(error) => err(to_file_error(
                    error,
                    Some(&file_path.to_string_lossy().to_string()),
                )),
            }
        })
    }

    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let pids: Vec<u32> = self
                .active_child_pids
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .drain()
                .collect();
            for pid in pids {
                kill_process_tree(pid);
            }
        })
    }
}

impl Shell for TokioEnv {
    fn exec<'a>(
        &'a self,
        command: String,
        options: ShellExecOptions,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecutionError>> {
        Box::pin(async move {
            if let Some(signal) = &options.abort_signal {
                if signal.is_cancelled() {
                    return err(ExecutionError::new(ExecutionErrorCode::Aborted, "aborted"));
                }
            }
            let timeout_seconds = resolve_timeout_seconds(options.timeout)?;

            let cwd = match &options.cwd {
                Some(cwd) => resolve_path(&self.cwd_text, cwd),
                None => self.cwd.clone(),
            };
            let shell_config = get_shell_config(self.shell_path.as_deref()).await?;
            if !path_exists(&cwd).await {
                return err(ExecutionError::new(
                    ExecutionErrorCode::SpawnError,
                    format!(
                        "Working directory does not exist: {}\nCannot execute bash commands.",
                        cwd.to_string_lossy()
                    ),
                ));
            }

            let mut cmd = Command::new(&shell_config.shell);
            let command_from_stdin = shell_config.command_transport_stdin;
            if command_from_stdin {
                cmd.args(&shell_config.args);
            } else {
                let mut args = shell_config.args.clone();
                args.push(command.clone());
                cmd.args(&args);
            }
            cmd.current_dir(&cwd)
                .stdin(if command_from_stdin {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(windows)]
            {
                // CREATE_NO_WINDOW。
                cmd.creation_flags(0x0800_0000);
            }
            #[cfg(unix)]
            {
                // 独立进程组,便于整组终止(对齐 TS detached)。
                cmd.process_group(0);
            }

            let inherit_env = options.inherit_env.unwrap_or(true);
            if !inherit_env {
                cmd.env_clear();
                if let Some(extra) = &options.env {
                    for (key, value) in extra {
                        cmd.env(key, value);
                    }
                }
            } else {
                if let Some(base) = &self.shell_env {
                    for (key, value) in base {
                        cmd.env(key, value);
                    }
                }
                if let Some(extra) = &options.env {
                    for (key, value) in extra {
                        cmd.env(key, value);
                    }
                }
            }

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => {
                    return err(ExecutionError::new(
                        ExecutionErrorCode::SpawnError,
                        error.to_string(),
                    ))
                }
            };
            let pid = child.id();
            if let Some(pid) = pid {
                self.active_child_pids
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(pid);
            }

            if command_from_stdin {
                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    let _ = stdin.write_all(command.as_bytes()).await;
                    let _ = stdin.shutdown().await;
                }
            }

            // 读流任务:分块触发回调并累积全量文本。
            let stdout_text: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
            let stderr_text: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
            let callback_error: Arc<Mutex<Option<ExecutionError>>> = Arc::new(Mutex::new(None));

            let mut reader_tasks = Vec::new();
            fn spawn_reader<S: tokio::io::AsyncRead + Unpin + Send + 'static>(
                mut stream: S,
                sink: Arc<Mutex<String>>,
                callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
                callback_error: Arc<Mutex<Option<ExecutionError>>>,
            ) -> tokio::task::JoinHandle<()> {
                tokio::spawn(async move {
                    let mut buffer = [0u8; 8192];
                    loop {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => {
                                let chunk = String::from_utf8_lossy(&buffer[..read]).to_string();
                                sink.lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .push_str(&chunk);
                                if let Some(callback) = &callback {
                                    let result = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| callback(chunk)),
                                    );
                                    if result.is_err() {
                                        *callback_error
                                            .lock()
                                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                            Some(ExecutionError::new(
                                                ExecutionErrorCode::CallbackError,
                                                "output callback panicked",
                                            ));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                })
            }
            if let Some(stream) = child.stdout.take() {
                reader_tasks.push(spawn_reader(
                    stream,
                    stdout_text.clone(),
                    options.on_stdout.clone(),
                    callback_error.clone(),
                ));
            }
            if let Some(stream) = child.stderr.take() {
                reader_tasks.push(spawn_reader(
                    stream,
                    stderr_text.clone(),
                    options.on_stderr.clone(),
                    callback_error.clone(),
                ));
            }

            // 中止监听:仅注册信号时 cancel 即杀进程树。
            let abort_task = match (pid, options.abort_signal.clone()) {
                (Some(pid), Some(signal)) => Some(tokio::spawn(async move {
                    signal.cancelled().await;
                    kill_process_tree(pid);
                })),
                _ => None,
            };

            let mut timed_out = false;
            let status = loop {
                let sleep = tokio::time::sleep(
                    timeout_seconds
                        .map(|seconds| std::time::Duration::from_secs_f64(seconds))
                        .unwrap_or(std::time::Duration::from_secs(u64::MAX / 2)),
                );
                tokio::pin!(sleep);
                tokio::select! {
                    result = child.wait() => break result,
                    _ = &mut sleep, if timeout_seconds.is_some() => {
                        timed_out = true;
                        if let Some(current_pid) = child.id() {
                            kill_process_tree(current_pid);
                        }
                        // 收尾宽限(TS EXIT_STDIO_GRACE_MS)。
                        tokio::time::sleep(std::time::Duration::from_millis(EXIT_STDIO_GRACE_MS)).await;
                    }
                }
            };

            for task in reader_tasks {
                let _ = task.await;
            }
            if let Some(task) = abort_task {
                task.abort();
            }
            if let Some(pid) = pid {
                self.active_child_pids
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&pid);
            }

            if let Some(error) = callback_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
            {
                return err(error);
            }

            if timed_out {
                return err(ExecutionError::new(
                    ExecutionErrorCode::Timeout,
                    format!("timeout:{}", options.timeout.unwrap_or_default()),
                ));
            }
            if let Some(signal) = &options.abort_signal {
                if signal.is_cancelled() {
                    return err(ExecutionError::new(ExecutionErrorCode::Aborted, "aborted"));
                }
            }
            let exit_code = match status {
                Ok(status) => status.code().unwrap_or(0),
                Err(error) => {
                    return err(ExecutionError::new(
                        ExecutionErrorCode::SpawnError,
                        error.to_string(),
                    ))
                }
            };
            let stdout = stdout_text
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            let stderr = stderr_text
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            ok(ExecOutcome {
                stdout,
                stderr,
                exit_code,
            })
        })
    }

    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
        FileSystem::cleanup(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness::types::ExecutionEnv;
    use futures::future::BoxFuture;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn file_system_round_trip() {
        let env = TokioEnv::new(std::env::temp_dir());
        let temp_dir = env.create_temp_dir(None, None).await.unwrap();
        let file_path = format!("{}/hello.txt", temp_dir.trim_end_matches('/'));

        env.write_file(
            file_path.clone(),
            FileContent::Text("alpha\nbeta\n".to_string()),
            None,
        )
        .await
        .unwrap();

        let text = env.read_text_file(file_path.clone(), None).await.unwrap();
        assert_eq!(text, "alpha\nbeta\n");

        let lines = env
            .read_text_lines(
                file_path.clone(),
                ReadTextLinesOptions {
                    max_lines: Some(1),
                    abort_signal: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(lines, vec!["alpha"]);

        assert!(env.exists(file_path.clone(), None).await.unwrap());
        let info = env.file_info(file_path.clone()).await.unwrap();
        assert_eq!(info.kind, FileKind::File);
        assert_eq!(info.size, 11);
        assert_eq!(info.name, "hello.txt");

        let entries = env.list_dir(temp_dir.clone(), None).await.unwrap();
        assert_eq!(entries.len(), 1);

        env.append_file(
            file_path.clone(),
            FileContent::Text("gamma".to_string()),
        )
        .await
        .unwrap();
        let text = env.read_text_file(file_path.clone(), None).await.unwrap();
        assert_eq!(text, "alpha\nbeta\ngamma");

        let canonical = env.canonical_path(file_path.clone(), None).await.unwrap();
        assert!(!canonical.is_empty());

        env.remove(file_path.clone(), RemoveOptions::default())
            .await
            .unwrap();
        assert!(!env.exists(file_path, None).await.unwrap());
        env.remove(temp_dir, RemoveOptions {
            recursive: Some(true),
            force: Some(true),
            abort_signal: None,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn write_file_creates_parent_directories() {
        let env = TokioEnv::new(std::env::temp_dir());
        let temp_dir = env.create_temp_dir(None, None).await.unwrap();
        let deep = format!("{}/a/b/c/file.txt", temp_dir.trim_end_matches('/'));
        env.write_file(deep.clone(), FileContent::Text("x".to_string()), None)
            .await
            .unwrap();
        assert!(env.exists(deep, None).await.unwrap());
        env.remove(
            temp_dir,
            RemoveOptions {
                recursive: Some(true),
                force: Some(true),
                abort_signal: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn missing_file_maps_to_not_found() {
        let env = TokioEnv::new(std::env::temp_dir());
        let error = env
            .read_text_file("definitely-missing-file.txt".to_string(), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, FileErrorCode::NotFound);
        assert!(env
            .exists("definitely-missing-file.txt".to_string(), None)
            .await
            .unwrap() == false);
    }

    #[cfg(unix)]
    fn shell_command() -> String {
        "echo hello; echo world >&2; exit 3".to_string()
    }

    #[cfg(windows)]
    fn shell_command() -> String {
        // Windows 上 shell 为 Git Bash(蓝本语义),bash 语法。
        "echo hello; echo world >&2; exit 3".to_string()
    }

    #[tokio::test]
    async fn exec_captures_output_and_exit_code() {
        let env = TokioEnv::new(std::env::temp_dir());
        let chunks = Arc::new(AtomicUsize::new(0));
        let counter = chunks.clone();
        let outcome = env
            .exec(
                shell_command(),
                ShellExecOptions {
                    on_stdout: Some(Arc::new(move |_| {
                        counter.fetch_add(1, Ordering::SeqCst);
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(outcome.stdout.contains("hello"), "{}", outcome.stdout);
        assert!(outcome.stderr.contains("world"), "{}", outcome.stderr);
        assert_eq!(outcome.exit_code, 3);
        assert!(chunks.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn exec_timeout_kills_command() {
        let env = TokioEnv::new(std::env::temp_dir());
        // 仅用内建,避免外部 sleep 缺失/PATH 差异。
        let command = "while true; do :; done".to_string();
        let error = env
            .exec(
                command,
                ShellExecOptions {
                    timeout: Some(1.0),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ExecutionErrorCode::Timeout);
    }

    #[tokio::test]
    async fn exec_abort_cancels_command() {
        let env = TokioEnv::new(std::env::temp_dir());
        let signal = AbortSignal::new();
        let command = "while true; do :; done".to_string();
        let exec_env: Arc<dyn ExecutionEnv> = Arc::new(env);
        let exec_signal = signal.clone();
        let handle = tokio::spawn(async move {
            exec_env
                .exec(
                    command,
                    ShellExecOptions {
                        abort_signal: Some(exec_signal),
                        ..Default::default()
                    },
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        signal.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("exec must finish after cancel")
            .unwrap();
        assert_eq!(result.unwrap_err().code, ExecutionErrorCode::Aborted);
    }

    #[tokio::test]
    async fn resolve_path_lexically_normalizes() {
        let env = TokioEnv::new(std::env::temp_dir());
        let base = env.cwd().to_string();
        let absolute = env
            .absolute_path("./sub/../file.txt".to_string(), None)
            .await
            .unwrap();
        let expected = Path::new(&base).join("file.txt");
        assert_eq!(
            Path::new(&absolute),
            Path::new(&expected),
            "got {absolute}, expected {expected:?}"
        );
        let joined = env
            .join_path(
                vec![base, "b".to_string(), "c.txt".to_string()],
                None,
            )
            .await
            .unwrap();
        assert!(Path::new(&joined).ends_with(Path::new("b").join("c.txt")), "{joined}");
    }
}

