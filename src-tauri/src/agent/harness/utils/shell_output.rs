//! shell 输出捕获:对齐 `packages/agent/src/harness/utils/shell-output.ts`。
//!
//! `executeShellWithCapture` 在 [`Shell::exec`] 之上维护尾部输出缓冲(默认
//! 2×50KB)、行/字节计数与溢出时的全量输出临时文件。TS 的 promise 写入链对应为
//! mpsc + 顺序写任务;`onChunk` 的惰性 `getProgress` 对应为触发时构造的进度快照。

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::truncate::{
    truncate_tail, TruncatedBy, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncationResult,
};
use crate::agent::harness::types::{
    err, ok, CreateTempFileOptions, ExecutionError, ExecutionErrorCode, ExecutionEnv,
    FileContent, Result, ShellExecOptions,
};

/// 单个输出块的捕获进度(对齐 TS `ShellCaptureProgress`)。
#[derive(Clone, Debug)]
pub struct ShellCaptureProgress {
    pub output: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<String>,
    pub last_line_bytes: usize,
}

/// 捕获选项(对齐 TS `ShellCaptureOptions`)。
#[derive(Default)]
pub struct ShellCaptureOptions {
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub inherit_env: Option<bool>,
    pub timeout: Option<f64>,
    pub abort_signal: Option<crate::agent::types::AbortSignal>,
    /// 每个清洗后的输出块触发一次,携带当前进度快照。
    pub on_chunk: Option<Arc<dyn Fn(&str, ShellCaptureProgress) + Send + Sync>>,
    /// 执行失败随捕获结果返回而不是失败 Result。
    pub return_execution_errors: bool,
}

impl std::fmt::Debug for ShellCaptureOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellCaptureOptions")
            .field("cwd", &self.cwd)
            .field("timeout", &self.timeout)
            .field("return_execution_errors", &self.return_execution_errors)
            .finish_non_exhaustive()
    }
}

/// 捕获结果(对齐 TS `ShellCaptureResult`)。
#[derive(Clone, Debug)]
pub struct ShellCaptureResult {
    pub output: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<String>,
    pub last_line_bytes: usize,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    pub execution_error: Option<ExecutionError>,
}

impl ShellCaptureResult {
    fn from_progress(
        progress: ShellCaptureProgress,
        exit_code: Option<i32>,
        cancelled: bool,
        execution_error: Option<ExecutionError>,
    ) -> Self {
        let truncated = progress.truncation.truncated;
        Self {
            output: progress.output,
            truncation: progress.truncation,
            full_output_path: progress.full_output_path,
            last_line_bytes: progress.last_line_bytes,
            exit_code,
            cancelled,
            truncated,
            execution_error,
        }
    }
}

/// 清洗二进制输出:仅保留可打印与常见空白码点(对齐 TS `sanitizeBinaryOutput`)。
pub fn sanitize_binary_output(text: &str) -> String {
    text.chars()
        .filter(|&code| {
            if code == '\t' || code == '\n' || code == '\r' {
                return true;
            }
            if (code as u32) <= 0x1F {
                return false;
            }
            if (0xFFF9..=0xFFFB).contains(&(code as u32)) {
                return false;
            }
            true
        })
        .collect()
}

/// 把文本裁剪到最近 max_bytes 个 UTF-8 字节(对齐 TS `trimToLastUtf8Bytes`)。
fn trim_to_last_utf8_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

#[derive(Debug, Default)]
struct CaptureState {
    tail_output: String,
    total_bytes: usize,
    completed_lines: usize,
    has_open_line: bool,
    current_line_bytes: usize,
    full_output_path: Option<String>,
    full_output_requested: bool,
    capture_error: Option<ExecutionError>,
}

impl CaptureState {
    fn create_progress(&self) -> ShellCaptureProgress {
        let tail_truncation = truncate_tail(&self.tail_output, super::truncate::TruncationOptions::default());
        let total_lines = self.completed_lines + usize::from(self.has_open_line);
        let truncated = total_lines > DEFAULT_MAX_LINES || self.total_bytes > DEFAULT_MAX_BYTES;
        let mut truncation = tail_truncation;
        truncation.truncated = truncated;
        truncation.truncated_by = if truncated {
            Some(truncation.truncated_by.unwrap_or(if self.total_bytes > DEFAULT_MAX_BYTES {
                TruncatedBy::Bytes
            } else {
                TruncatedBy::Lines
            }))
        } else {
            None
        };
        truncation.total_lines = total_lines;
        truncation.total_bytes = self.total_bytes;
        ShellCaptureProgress {
            output: if truncated {
                truncation.content.clone()
            } else {
                self.tail_output.clone()
            },
            truncation,
            full_output_path: self.full_output_path.clone(),
            last_line_bytes: self.current_line_bytes,
        }
    }
}

enum WriterCommand {
    EnsureFullOutput { initial_content: String },
    Append { text: String },
}

async fn run_writer(
    env: Arc<dyn ExecutionEnv>,
    state: Arc<Mutex<CaptureState>>,
    mut receiver: mpsc::UnboundedReceiver<WriterCommand>,
) -> Result<(), ExecutionError> {
    // 文件错误 → ExecutionError(unknown),与 TS toExecutionError 语义一致。
    fn to_exec(error: crate::agent::harness::types::FileError) -> ExecutionError {
        ExecutionError::new(ExecutionErrorCode::Unknown, error.message)
    }
    let mut write_result: Result<(), ExecutionError> = ok(());
    while let Some(command) = receiver.recv().await {
        if write_result.is_err() {
            // 失败后继续排空通道以保持顺序语义,但不再写文件。
            continue;
        }
        match command {
            WriterCommand::EnsureFullOutput { initial_content } => {
                let temp_file = env
                    .create_temp_file(CreateTempFileOptions {
                        prefix: Some("bash-".to_string()),
                        suffix: Some(".log".to_string()),
                        abort_signal: None,
                    })
                    .await;
                let path = match temp_file {
                    Ok(path) => path,
                    Err(error) => {
                        write_result = Err(to_exec(error));
                        continue;
                    }
                };
                match env
                    .append_file(path.clone(), FileContent::Text(initial_content))
                    .await
                {
                    Ok(()) => {
                        state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .full_output_path = Some(path);
                    }
                    Err(error) => write_result = Err(to_exec(error)),
                }
            }
            WriterCommand::Append { text } => {
                let path = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .full_output_path
                    .clone();
                if let Some(path) = path {
                    if let Err(error) = env.append_file(path, FileContent::Text(text)).await {
                        write_result = Err(to_exec(error));
                    }
                }
            }
        }
    }
    write_result
}

/// 捕获式 shell 执行(对齐 TS `executeShellWithCapture`)。
pub async fn execute_shell_with_capture(
    env: Arc<dyn ExecutionEnv>,
    command: &str,
    options: ShellCaptureOptions,
) -> Result<ShellCaptureResult, ExecutionError> {
    let state = Arc::new(Mutex::new(CaptureState::default()));
    let accepting_output = Arc::new(AtomicBool::new(true));
    let (writer_tx, writer_rx) = mpsc::unbounded_channel::<WriterCommand>();

    let writer_task = tokio::spawn(run_writer(
        env.clone(),
        state.clone(),
        writer_rx,
    ));

    let on_chunk: Arc<dyn Fn(String) + Send + Sync> = {
        let state = state.clone();
        let accepting_output = accepting_output.clone();
        let writer_tx = writer_tx.clone();
        let on_chunk_cb = options.on_chunk.clone();
        Arc::new(move |chunk: String| {
            if !accepting_output.load(Ordering::SeqCst) {
                return;
            }
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                let text = sanitize_binary_output(&chunk).replace('\r', "");
                let text_bytes = text.len();
                let mut st = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if st.capture_error.is_some() {
                    return;
                }
                st.total_bytes += text_bytes;
                let newline_count = text.matches('\n').count();
                st.completed_lines += newline_count;
                match text.rfind('\n') {
                    Some(last_newline) => {
                        let trailing = &text[last_newline + 1..];
                        st.current_line_bytes = trailing.len();
                        st.has_open_line = !trailing.is_empty();
                    }
                    None if !text.is_empty() => {
                        st.current_line_bytes += text_bytes;
                        st.has_open_line = true;
                    }
                    None => {}
                }
                st.tail_output.push_str(&text);
                let total_lines = st.completed_lines + usize::from(st.has_open_line);
                if (st.total_bytes > DEFAULT_MAX_BYTES || total_lines > DEFAULT_MAX_LINES)
                    && !st.full_output_requested
                {
                    st.full_output_requested = true;
                    let initial = st.tail_output.clone();
                    let _ = writer_tx.send(WriterCommand::EnsureFullOutput {
                        initial_content: initial,
                    });
                } else if st.full_output_requested {
                    let _ = writer_tx.send(WriterCommand::Append { text: text.clone() });
                }
                st.tail_output = trim_to_last_utf8_bytes(&st.tail_output.clone(), DEFAULT_MAX_BYTES * 2);
                if let Some(on_chunk_cb) = &on_chunk_cb {
                    let progress = st.create_progress();
                    on_chunk_cb(&text, progress);
                }
            }));
            if result.is_err() {
                let mut st = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                st.capture_error = Some(ExecutionError::new(
                    ExecutionErrorCode::Unknown,
                    "output capture callback panicked",
                ));
            }
        })
    };

    let exec_options = ShellExecOptions {
        cwd: options.cwd.clone(),
        env: options.env.clone(),
        inherit_env: options.inherit_env,
        timeout: options.timeout,
        abort_signal: options.abort_signal.clone(),
        on_stdout: Some(on_chunk.clone()),
        on_stderr: Some(on_chunk),
    };

    let exec_result = env.exec(command.to_string(), exec_options).await;
    accepting_output.store(false, Ordering::SeqCst);

    // 终止后的兜底:若发生截断但未建立全量文件,现在补建(TS ensureFullOutputFile)。
    {
        let st = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let progress = st.create_progress();
        if progress.truncation.truncated && !st.full_output_requested && st.capture_error.is_none() {
            let initial = st.tail_output.clone();
            let _ = writer_tx.send(WriterCommand::EnsureFullOutput {
                initial_content: initial,
            });
        }
    }
    drop(writer_tx);
    let write_result = writer_task
        .await
        .unwrap_or_else(|error| Err(to_execution_error(error.to_string())));

    if let Err(error) = write_result {
        return err(error);
    }
    {
        let st = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(error) = &st.capture_error {
            return err(error.clone());
        }
    }
    let progress = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .create_progress();

    match exec_result {
        Err(exec_error) => {
            let aborted = exec_error.code == ExecutionErrorCode::Aborted
                || options
                    .abort_signal
                    .as_ref()
                    .map(|signal| signal.is_cancelled())
                    .unwrap_or(false);
            if aborted {
                return ok(ShellCaptureResult::from_progress(progress, None, true, None));
            }
            if options.return_execution_errors {
                return ok(ShellCaptureResult::from_progress(
                    progress,
                    None,
                    false,
                    Some(exec_error),
                ));
            }
            err(exec_error)
        }
        Ok(outcome) => {
            let cancelled = options
                .abort_signal
                .as_ref()
                .map(|signal| signal.is_cancelled())
                .unwrap_or(false);
            ok(ShellCaptureResult::from_progress(
                progress,
                if cancelled { None } else { Some(outcome.exit_code) },
                cancelled,
                None,
            ))
        }
    }
}

fn to_execution_error(message: String) -> ExecutionError {
    ExecutionError::new(ExecutionErrorCode::Unknown, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn sanitize_filters_control_and_interlinear_chars() {
        assert_eq!(sanitize_binary_output("a\u{0}b\u{1}c"), "abc");
        assert_eq!(sanitize_binary_output("keep\ttab\nnl\rcr"), "keep\ttab\nnl\rcr");
        assert_eq!(sanitize_binary_output("a\u{FFF9}b\u{FFFB}c"), "abc");
        assert_eq!(sanitize_binary_output("é中\u{7F}"), "é中\u{7F}");
    }

    #[test]
    fn trim_to_last_utf8_bytes_respects_boundaries() {
        let text = "aé🙂bc";
        // 🙂 = 4 字节;é = 2 字节。
        assert_eq!(trim_to_last_utf8_bytes(text, 100), text);
        let trimmed = trim_to_last_utf8_bytes(text, 5);
        assert!(trimmed.chars().eq("bc".chars()) || trimmed.ends_with("bc"), "got {trimmed}");
        assert!(trimmed.len() <= 5);
    }

    /// 内存 ExecutionEnv 固定输出(供捕获测试)。
    struct EchoEnv;

    impl crate::agent::harness::types::FileSystem for EchoEnv {
        fn cwd(&self) -> &str {
            "/"
        }

        fn absolute_path<'a>(&'a self, path: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(path) })
        }

        fn join_path<'a>(&'a self, parts: Vec<String>, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(parts.join("/")) })
        }

        fn read_text_file<'a>(&'a self, path: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(path) })
        }

        fn read_text_lines<'a>(&'a self, path: String, _options: crate::agent::harness::types::ReadTextLinesOptions) -> futures::future::BoxFuture<'a, Result<Vec<String>, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(vec![path]) })
        }

        fn read_binary_file<'a>(&'a self, path: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<Vec<u8>, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(path.into_bytes()) })
        }

        fn write_file<'a>(&'a self, _path: String, _content: FileContent, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(()) })
        }

        fn append_file<'a>(&'a self, _path: String, _content: FileContent) -> futures::future::BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(()) })
        }

        fn rename_file<'a>(&'a self, _source: String, _destination: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(()) })
        }

        fn file_info<'a>(&'a self, path: String) -> futures::future::BoxFuture<'a, Result<crate::agent::harness::types::FileInfo, crate::agent::harness::types::FileError>> {
            Box::pin(async move {
                Ok(crate::agent::harness::types::FileInfo {
                    name: path,
                    path: String::new(),
                    kind: crate::agent::harness::types::FileKind::File,
                    size: 0,
                    mtime_ms: 0.0,
                })
            })
        }

        fn list_dir<'a>(&'a self, _path: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<Vec<crate::agent::harness::types::FileInfo>, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn canonical_path<'a>(&'a self, path: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(path) })
        }

        fn exists<'a>(&'a self, _path: String, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<bool, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(true) })
        }

        fn create_dir<'a>(&'a self, _path: String, _options: crate::agent::harness::types::CreateDirOptions) -> futures::future::BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(()) })
        }

        fn remove<'a>(&'a self, _path: String, _options: crate::agent::harness::types::RemoveOptions) -> futures::future::BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok(()) })
        }

        fn create_temp_dir<'a>(&'a self, _prefix: Option<String>, _abort: Option<crate::agent::types::AbortSignal>) -> futures::future::BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok("/tmp/echo-env".to_string()) })
        }

        fn create_temp_file<'a>(&'a self, _options: CreateTempFileOptions) -> futures::future::BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
            Box::pin(async move { Ok("/tmp/echo-env/out.log".to_string()) })
        }

        fn cleanup<'a>(&'a self) -> futures::future::BoxFuture<'a, ()> {
            Box::pin(async move {})
        }
    }

    impl crate::agent::harness::types::Shell for EchoEnv {
        fn exec<'a>(
            &'a self,
            _command: String,
            options: ShellExecOptions,
        ) -> futures::future::BoxFuture<'a, Result<crate::agent::harness::types::ExecOutcome, ExecutionError>> {
            Box::pin(async move {
                if let Some(on_stdout) = &options.on_stdout {
                    on_stdout("hello\nworld\n".to_string());
                }
                Ok(crate::agent::harness::types::ExecOutcome {
                    stdout: "hello\nworld\n".to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            })
        }

        fn cleanup<'a>(&'a self) -> futures::future::BoxFuture<'a, ()> {
            Box::pin(async move {})
        }
    }

    #[tokio::test]
    async fn captures_output_and_counts_lines() {
        let env: Arc<dyn ExecutionEnv> = Arc::new(EchoEnv);
        let chunks = Arc::new(AtomicUsize::new(0));
        let counter = chunks.clone();
        let result = execute_shell_with_capture(
            env,
            "echo hi",
            ShellCaptureOptions {
                on_chunk: Some(Arc::new(move |text, progress| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    assert!(text.contains("hello") || text.contains("world"));
                    assert!(!progress.truncation.truncated);
                })),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(result.output, "hello\nworld\n");
        assert!(!result.truncated);
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.cancelled);
        assert_eq!(result.truncation.total_lines, 2);
        assert_eq!(chunks.load(Ordering::SeqCst), 1);
    }
}
