//! 内置 AgentHarness 的无头组装胶水(wiki 生成与资源库安全扫描共享):
//! 内存会话、provider 流桥接、只读工具调用预算、harness 构造、prompt
//! 超时/取消与逐请求用量采集。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::agent::harness::agent_harness::{
    AgentHarness, AgentHarnessOptions, RetryPolicy, RunOutcome,
};
use crate::agent::harness::events::{HarnessEvent, HarnessEventType, UsageEvent};
use crate::agent::harness::runtime::harness_tool_from_core;
use crate::agent::harness::session::memory::InMemorySessionStorage;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::SessionMetadata;
use crate::agent::harness::tools::index::{
    create_find_tool, create_grep_tool, create_ls_tool, create_read_tool,
};
use crate::agent::harness::types::{AgentHarnessTool, ExecutionEnv, SimpleError};
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::stream_simple;
use crate::agent::llm::types::{
    AssistantContent, AssistantMessage, Model, SimpleStreamOptions, StopReason,
};
use crate::agent::types::{AgentTool, QueueMode, StreamFn, ToolExecutionError, ToolExecutionMode};
use crate::commands::usage::insert_usage_row;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::AiUsageRecord;
use crate::time_util::now_ts;

/// 单次 agent prompt 的总超时(与 ACP prompt 上限一致)
const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub(crate) fn memory_session() -> Session {
    Session::new(Arc::new(InMemorySessionStorage::new(SessionMetadata {
        id: uuid_v7(),
        created_at: crate::agent::agent_loop::now_ms(),
        parent_session_id: None,
    })))
}

pub(crate) fn builtin_stream_fn(api_key: String, cancel: CancellationToken) -> StreamFn {
    Arc::new(move |model, context, options| {
        let api_key = api_key.clone();
        let cancel = cancel.clone();
        Box::pin(async move {
            let base = options.unwrap_or_default();
            stream_simple(
                model,
                context,
                Some(SimpleStreamOptions {
                    api_key: Some(api_key),
                    ..base
                }),
                Some(cancel),
            )
        })
    })
}

fn budget_tool(
    tool: AgentTool,
    budget: Arc<std::sync::atomic::AtomicUsize>,
    limit: usize,
) -> AgentTool {
    use std::sync::atomic::Ordering;

    let execute = tool.execute.clone();
    let name = tool.name.clone();
    AgentTool {
        execute: Arc::new(move |tool_call_id, params, signal, on_update| {
            let execute = execute.clone();
            let budget = budget.clone();
            let name = name.clone();
            Box::pin(async move {
                let used = budget.fetch_add(1, Ordering::SeqCst) + 1;
                if used > limit {
                    return Err(ToolExecutionError::from(SimpleError::new(format!(
                        "{name} tool budget exceeded: at most {limit} repository exploration calls are allowed"
                    ))));
                }
                execute(tool_call_id, params, signal, on_update).await
            })
        }),
        ..tool
    }
}

/// 只读工具集(read/grep/find/ls),统一包装调用次数预算
pub(crate) fn read_tools(env: Arc<dyn ExecutionEnv>, limit: usize) -> Vec<AgentHarnessTool> {
    let budget = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    [
        create_read_tool(env.clone(), None),
        create_grep_tool(env.clone()),
        create_find_tool(env.clone()),
        create_ls_tool(env),
    ]
    .into_iter()
    .map(|tool| budget_tool(tool, budget.clone(), limit))
    .map(harness_tool_from_core)
    .collect()
}

/// 逐 LLM 请求落库(harness 每轮 assistant 消息一个 UsageEvent)
pub(crate) fn usage_record(db: &Db, task_type: &str, model: &str, event: &UsageEvent) {
    let usage = &event.usage;
    let record = AiUsageRecord {
        task_type: task_type.to_string(),
        model: model.to_string(),
        input_tokens: Some(usage.input),
        output_tokens: Some(usage.output),
        total_tokens: Some(usage.total_tokens),
        duration_ms: event.elapsed_ms,
        cached_tokens: Some(usage.cache_read),
    };
    if let Ok(conn) = db.0.lock() {
        let _ = insert_usage_row(&conn, &record, now_ts());
    }
}

pub(crate) fn record_collected_usage(db: &Db, task_type: &str, model: &str, usages: &[UsageEvent]) {
    for usage in usages {
        usage_record(db, task_type, model, usage);
    }
}

pub(crate) fn assistant_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_outcome_message(outcome: RunOutcome) -> AppResult<AssistantMessage> {
    match outcome {
        RunOutcome::Completed { final_message, .. }
            if !matches!(
                final_message.stop_reason,
                StopReason::Error | StopReason::Aborted | StopReason::Length
            ) =>
        {
            Ok(final_message)
        }
        RunOutcome::Completed { final_message, .. } | RunOutcome::Aborted { final_message, .. } => {
            Err(AppError::coded(
                ErrorCode::AiRequestFailed,
                final_message
                    .error_message
                    .unwrap_or_else(|| format!("agent stopped: {:?}", final_message.stop_reason)),
            ))
        }
        RunOutcome::Failed { error, .. } => {
            Err(AppError::coded(ErrorCode::AiRequestFailed, error.message))
        }
        RunOutcome::Suspended { .. } => Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "agent suspended",
        )),
    }
}

pub(crate) async fn create_harness(
    model: Model,
    stream_fn: StreamFn,
    tools: Vec<AgentHarnessTool>,
    system_prompt: String,
    thinking: crate::agent::llm::types::ModelThinkingLevel,
) -> AppResult<AgentHarness> {
    let (harness, suspended) = AgentHarness::create(AgentHarnessOptions {
        session: memory_session(),
        stream_fn,
        thinking_level: Some(thinking),
        model,
        active_tool_names: None,
        tools,
        tool_context: None,
        system_prompt: Some(system_prompt),
        resources: Default::default(),
        stream_options: Default::default(),
        retry: Some(RetryPolicy {
            enabled: true,
            max_retries: 2,
            base_delay_ms: 1000,
        }),
        compaction: None,
        steering_mode: QueueMode::OneAtATime,
        follow_up_mode: QueueMode::OneAtATime,
        tool_execution: ToolExecutionMode::Sequential,
        telemetry_context: None,
    })
    .await
    .map_err(|error| AppError::coded(ErrorCode::AiRequestFailed, error.to_string()))?;
    if !suspended.is_empty() {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "unexpected suspended in-memory session",
        ));
    }
    Ok(harness)
}

/// prompt 执行:`cancel` 触发 abort,或总超时;返回最终 assistant 消息
pub(crate) async fn prompt_with_timeout(
    harness: Arc<AgentHarness>,
    prompt: String,
    cancel: &CancellationToken,
    request_cancel: &CancellationToken,
) -> AppResult<AssistantMessage> {
    let abort_harness = harness.clone();
    let cancel_watch = cancel.clone();
    let request_watch = request_cancel.clone();
    let watcher = tokio::spawn(async move {
        cancel_watch.cancelled().await;
        request_watch.cancel();
        let _ = abort_harness.abort().await;
    });
    let result = tokio::time::timeout(RUN_TIMEOUT, harness.prompt(prompt)).await;
    watcher.abort();
    match result {
        Ok(Ok(outcome)) => run_outcome_message(outcome),
        Ok(Err(error)) => Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            error.to_string(),
        )),
        Err(_) => {
            request_cancel.cancel();
            let _ = harness.abort().await;
            Err(AppError::coded(
                ErrorCode::AiRequestFailed,
                "agent timed out",
            ))
        }
    }
}

pub(crate) async fn collect_usage_events(harness: &AgentHarness) -> Arc<Mutex<Vec<UsageEvent>>> {
    let usages = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let listener = usages.clone();
    let _subscription = harness.on_event(
        HarnessEventType::Usage,
        Arc::new(move |event| {
            if let HarnessEvent::Usage(event) = event {
                listener.lock().unwrap().push(event.clone());
            }
        }),
    );
    usages
}
