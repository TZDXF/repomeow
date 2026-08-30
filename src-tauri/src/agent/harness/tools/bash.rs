//! bash 工具:对齐 `packages/agent/src/harness/tools/bash.ts`。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::agent::harness::types::{ExecutionEnv, SimpleError};
use crate::agent::harness::utils::shell_output::{
    execute_shell_with_capture, ShellCaptureOptions, ShellCaptureProgress,
};
use crate::agent::harness::utils::truncate::{
    format_size, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, TruncationResult,
};
use crate::agent::types::{AgentTool, AgentToolResult, AbortSignal, ToolExecutionError};

/// bash 最长超时(秒;对齐 TS `MAX_TIMEOUT_SECONDS`)。
pub const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;

const BASH_UPDATE_THROTTLE_MS: u64 = 100;

/// bash 工具详情(对齐 TS `BashToolDetails`)。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// bash 执行描述(prepare 钩子可改写;对齐 TS `BashExecution`)。
#[derive(Clone, Debug)]
pub struct BashExecution {
    pub command: String,
    pub cwd: String,
    pub env: std::collections::HashMap<String, String>,
    pub inherit_env: bool,
}

/// prepare 钩子(执行前观测/改写命令;对齐 TS `BashPrepare`)。
pub type BashPrepare =
    Arc<dyn Fn(&mut BashExecution, Option<AbortSignal>) -> BoxFuture<'static, ()> + Send + Sync>;

/// bash 工具选项(对齐 TS `BashToolOptions`)。
#[derive(Clone, Default)]
pub struct BashToolOptions {
    pub command_prefix: Option<String>,
    pub prepare: Option<BashPrepare>,
}

/// bash 工具参数(对齐 TS `BashToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashToolInput {
    pub command: String,
    pub timeout: Option<f64>,
}

struct UpdateThrottle {
    last_update_at: AtomicU64,
    pending: Mutex<bool>,
}

impl UpdateThrottle {
    fn new() -> Self {
        Self {
            last_update_at: AtomicU64::new(0),
            pending: Mutex::new(false),
        }
    }

    /// 立即或标记节流;返回是否本次应立即发出。
    fn schedule(&self) -> bool {
        let now = now_ms_u64();
        let last = self.last_update_at.load(Ordering::SeqCst);
        let delay = BASH_UPDATE_THROTTLE_MS.saturating_sub(now.saturating_sub(last));
        if delay == 0 {
            self.last_update_at.store(now, Ordering::SeqCst);
            *self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
            true
        } else {
            *self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            false
        }
    }

    /// 终态冲刷:无论节流都允许发出。
    fn flush(&self) -> bool {
        self.last_update_at.store(now_ms_u64(), Ordering::SeqCst);
        let mut pending = self.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let was_pending = *pending;
        *pending = false;
        was_pending || true
    }
}

fn now_ms_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn validate_timeout(timeout: Option<f64>) -> Result<(), ToolExecutionError> {
    let Some(timeout) = timeout else {
        return Ok(());
    };
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(ToolExecutionError::from(SimpleError::new(
            "Invalid timeout: must be a finite number of seconds",
        )));
    }
    if timeout > MAX_TIMEOUT_SECONDS {
        return Err(ToolExecutionError::from(SimpleError::new(format!(
            "Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
        ))));
    }
    Ok(())
}

/// 创建 bash 工具(尾截断保留最近输出,溢出写临时文件;返回 core AgentTool)。
pub fn create_bash_tool(env: Arc<dyn ExecutionEnv>, options: Option<BashToolOptions>) -> AgentTool {
    let options = options.unwrap_or_default();
    AgentTool {
        name: "bash".to_string(),
        label: "bash".to_string(),
        description: format!(
            "Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds.",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, no default timeout)"
                }
            },
            "required": ["command"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, params, signal, on_update| {
            let env = env.clone();
            let options = options.clone();
            Box::pin(async move {
                let input: BashToolInput = serde_json::from_value(params)
                    .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))?;
                validate_timeout(input.timeout)?;

                let mut execution = BashExecution {
                    command: match &options.command_prefix {
                        Some(prefix) => format!("{}\n{}", prefix, input.command),
                        None => input.command.clone(),
                    },
                    cwd: env.cwd().to_string(),
                    env: std::collections::HashMap::new(),
                    inherit_env: true,
                };
                if let Some(prepare) = &options.prepare {
                    prepare(&mut execution, signal.clone()).await;
                }

                let throttle = Arc::new(UpdateThrottle::new());
                let on_chunk: Arc<dyn Fn(&str, ShellCaptureProgress) + Send + Sync> = {
                    let throttle = throttle.clone();
                    let on_update = on_update.clone();
                    Arc::new(move |_text, progress| {
                        if let Some(on_update) = on_update.as_ref() {
                            if throttle.schedule() {
                                on_update(partial_result_from_progress(&progress));
                            }
                        }
                    })
                };

                if let Some(on_update) = &on_update {
                    // 初始空更新(对齐 TS onUpdate?.({ content: [], details: undefined }))。
                    on_update(AgentToolResult {
                        content: vec![],
                        ..Default::default()
                    });
                }

                let capture = execute_shell_with_capture(
                    env.clone(),
                    &execution.command,
                    ShellCaptureOptions {
                        cwd: Some(execution.cwd.clone()),
                        env: if execution.env.is_empty() {
                            None
                        } else {
                            Some(execution.env.clone())
                        },
                        inherit_env: Some(execution.inherit_env),
                        timeout: input.timeout,
                        abort_signal: signal.clone(),
                        on_chunk: Some(on_chunk),
                        return_execution_errors: true,
                    },
                )
                .await;

                // 终态冲刷节流中的更新。
                let final_progress = capture.as_ref().ok().map(|capture| ShellCaptureProgress {
                    output: capture.output.clone(),
                    truncation: capture.truncation.clone(),
                    full_output_path: capture.full_output_path.clone(),
                    last_line_bytes: capture.last_line_bytes,
                });
                if let (Some(on_update), Some(progress)) = (&on_update, &final_progress) {
                    if throttle.flush() {
                        on_update(partial_result_from_progress(progress));
                    }
                }

                let capture = capture?;
                let mut output_text = capture.output.clone();
                let mut details: Option<Value> = None;
                if capture.truncation.truncated {
                    details = Some(
                        serde_json::to_value(BashToolDetails {
                            truncation: Some(capture.truncation.clone()),
                            full_output_path: capture.full_output_path.clone(),
                        })
                        .unwrap_or(Value::Null),
                    );
                    let start_line = capture.truncation.total_lines - capture.truncation.output_lines + 1;
                    let end_line = capture.truncation.total_lines;
                    if capture.truncation.last_line_partial {
                        let last_line_size = format_size(capture.last_line_bytes);
                        output_text.push_str(&format!(
                            "\n\n[Showing last {} of line {end_line} (line is {last_line_size}). Full output: {}]",
                            format_size(capture.truncation.output_bytes),
                            capture.full_output_path.clone().unwrap_or_default()
                        ));
                    } else if capture.truncation.truncated_by == Some(TruncatedBy::Lines) {
                        output_text.push_str(&format!(
                            "\n\n[Showing lines {start_line}-{end_line} of {}. Full output: {}]",
                            capture.truncation.total_lines,
                            capture.full_output_path.clone().unwrap_or_default()
                        ));
                    } else {
                        output_text.push_str(&format!(
                            "\n\n[Showing lines {start_line}-{end_line} of {} ({} limit). Full output: {}]",
                            capture.truncation.total_lines,
                            format_size(DEFAULT_MAX_BYTES),
                            capture.full_output_path.clone().unwrap_or_default()
                        ));
                    }
                }

                let append_status = |status: String| -> String {
                    if output_text.is_empty() {
                        status
                    } else {
                        format!("{output_text}\n\n{status}")
                    }
                };
                if capture.cancelled {
                    return Err(ToolExecutionError::from(SimpleError::new(append_status(
                        "Command aborted".to_string(),
                    ))));
                }
                if let Some(execution_error) = &capture.execution_error {
                    if execution_error.code == crate::agent::harness::types::ExecutionErrorCode::Timeout {
                        return Err(ToolExecutionError::from(SimpleError::new(append_status(format!(
                            "Command timed out after {} seconds",
                            input
                                .timeout
                                .map(|t| t.to_string())
                                .unwrap_or_else(|| "unknown".to_string())
                        )))));
                    }
                    return Err(ToolExecutionError::from(SimpleError::new(
                        execution_error.message.clone(),
                    )));
                }
                let exit_code = capture.exit_code.unwrap_or(0);
                if exit_code != 0 {
                    return Err(ToolExecutionError::from(SimpleError::new(append_status(
                        format!("Command exited with code {exit_code}"),
                    ))));
                }
                Ok(AgentToolResult {
                    content: vec![crate::agent::types::TextOrImageContent::text(if output_text.is_empty() {
                        "(no output)".to_string()
                    } else {
                        output_text
                    })],
                    details: details.unwrap_or(Value::Null),
                    ..Default::default()
                })
            })
        }),
    }
}

fn partial_result_from_progress(progress: &ShellCaptureProgress) -> AgentToolResult {
    AgentToolResult {
        content: vec![crate::agent::types::TextOrImageContent::text(progress.output.clone())],
        details: serde_json::to_value(BashToolDetails {
            truncation: if progress.truncation.truncated {
                Some(progress.truncation.clone())
            } else {
                None
            },
            full_output_path: progress.full_output_path.clone(),
        })
        .unwrap_or(Value::Null),
        ..Default::default()
    }
}
