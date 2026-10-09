//! bash 工具:对齐 `packages/agent/src/harness/tools/bash.ts`。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio::runtime::Handle;

use crate::agent::harness::types::{ExecutionEnv, SimpleError};
use crate::agent::harness::utils::adaptive_publisher::AdaptivePublisher;
use crate::agent::harness::utils::shell_output::{
    execute_shell_with_capture, ShellCaptureOptions, ShellCaptureProgress,
};
use crate::agent::harness::utils::truncate::{
    format_size, TruncatedBy, TruncationResult, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use crate::agent::types::{AbortSignal, AgentTool, AgentToolResult, ToolExecutionError};

/// bash 最长超时(秒;对齐 TS `MAX_TIMEOUT_SECONDS`)。
pub const MAX_TIMEOUT_SECONDS: f64 = 2_147_483_647.0 / 1000.0;

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

type ProgressSink = Arc<dyn Fn(&ShellCaptureProgress) + Send + Sync>;

enum PublishAction {
    Immediate,
    Delayed { generation: u64, delay: Duration },
    Skip,
}

struct SharedPublisherState {
    publisher: AdaptivePublisher,
    progress: Option<ShellCaptureProgress>,
    generation: u64,
    emit_in_flight: bool,
}

/// 将 TS OutputCapture/AdaptivePublisher 的“只发最新快照”模式桥接到同步回调。
struct UpdatePublisher {
    state: Mutex<SharedPublisherState>,
}

impl UpdatePublisher {
    fn new() -> Self {
        Self {
            state: Mutex::new(SharedPublisherState {
                publisher: AdaptivePublisher::new(),
                progress: None,
                generation: 0,
                emit_in_flight: false,
            }),
        }
    }

    fn record_progress(&self, progress: ShellCaptureProgress) -> PublishAction {
        let mut state = self.lock();
        state.progress = Some(progress);
        if state.emit_in_flight {
            return PublishAction::Skip;
        }
        let size = state
            .progress
            .as_ref()
            .map_or(0, |progress| progress.output.len() as u64);
        match state.publisher.mark_dirty(size) {
            Some(delay) => PublishAction::Delayed {
                generation: state.generation,
                delay,
            },
            None => {
                state.emit_in_flight = true;
                PublishAction::Immediate
            }
        }
    }

    fn publish_progress(self: &Arc<Self>, progress: ShellCaptureProgress, sink: ProgressSink) {
        match self.record_progress(progress) {
            PublishAction::Skip => {}
            PublishAction::Immediate => self.emit(sink),
            PublishAction::Delayed { generation, delay } => {
                if let Ok(handle) = Handle::try_current() {
                    let publisher = self.clone();
                    handle.spawn(async move {
                        tokio::time::sleep(delay).await;
                        publisher.flush_if_current(generation, sink);
                    });
                }
            }
        }
    }

    fn flush_if_current(self: &Arc<Self>, expected_generation: u64, sink: ProgressSink) {
        {
            let mut state = self.lock();
            if state.generation != expected_generation
                || state.emit_in_flight
                || !state.publisher.should_flush()
            {
                return;
            }
            state.emit_in_flight = true;
        }
        self.emit(sink);
    }

    /// 终态强制发布;generation 失效所有仍在等待的尾随任务。
    fn flush_final(&self, progress: Option<ShellCaptureProgress>, sink: ProgressSink) {
        {
            let mut state = self.lock();
            if let Some(progress) = progress {
                state.progress = Some(progress);
            }
            state.generation += 1;
            state.emit_in_flight = true;
        }
        self.emit(sink);
    }

    fn emit(&self, sink: ProgressSink) {
        let progress = {
            let mut state = self.lock();
            let progress = state.progress.clone();
            let size = progress
                .as_ref()
                .map_or(0, |progress| progress.output.len() as u64);
            state.publisher.record_flush(size);
            state.generation += 1;
            state.emit_in_flight = false;
            progress
        };
        if let Some(progress) = progress {
            sink(&progress);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SharedPublisherState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
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
        replay: None,
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

                let publisher = Arc::new(UpdatePublisher::new());
                let on_chunk: Arc<dyn Fn(&str, ShellCaptureProgress) + Send + Sync> = {
                    let publisher = publisher.clone();
                    let on_update = on_update.clone();
                    Arc::new(move |_text, progress| {
                        if let Some(on_update) = on_update.as_ref() {
                            let on_update = on_update.clone();
                            let sink: ProgressSink = Arc::new(move |progress| {
                                on_update(partial_result_from_progress(progress));
                            });
                            publisher.publish_progress(progress, sink);
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
                if let Some(on_update) = on_update.as_ref() {
                    let on_update = on_update.clone();
                    // 终态强制冲刷;执行失败时也发布已捕获的最新输出。
                    let sink: ProgressSink = Arc::new(move |progress| {
                        on_update(partial_result_from_progress(progress));
                    });
                    publisher.flush_final(final_progress, sink);
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
        content: vec![crate::agent::types::TextOrImageContent::text(
            progress.output.clone(),
        )],
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

/// 便捷构造:以 cwd 构造 bash env 并创建工具(powershell 同款便捷入口)。
pub fn create_local_bash_tool(
    cwd: impl Into<std::path::PathBuf>,
    options: Option<BashToolOptions>,
) -> AgentTool {
    let env: Arc<dyn ExecutionEnv> = Arc::new(crate::agent::harness::env::TokioEnv::new(cwd));
    create_bash_tool(env, options)
}
