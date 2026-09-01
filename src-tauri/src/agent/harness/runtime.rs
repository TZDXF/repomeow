//! harness 运行时:在 scaffold record 契约上接线 prompt/abort/队列/usage/事件。
//!
//! 职责边界(对齐 pi 源码语义,蓝本 AgentHarness 本身未提供实现,运行语义取自
//! `agent-loop.ts` 与 coding-agent `AgentSession`):
//! - 每次运行构造一个临时 core `Agent` 作为引擎;transcript 的权威真值是
//!   session(经 context.rs 从分支条目重建),引擎只在 run 期间承载在途状态。
//! - 镜像监听器把 AgentEvent 翻译为 session 条目/记录与 [`HarnessEvent`]。
//! - 队列以 `QueueEnqueued`/`QueueCancelled` 记录持久化,经 loop 的
//!   `get_steering_messages`/`get_follow_up_messages` 钩子在既有 drain 点消费,
//!   消费时以 provisioned id 物化为 transcript 条目。
//! - 重试对齐 `_prepareRetry`:失败 assistant 保留在 session、从引擎移除,
//!   退避后走 continuation(`Agent::continue_run`)。
//!
//! 已知偏差(相对未来 format-4 设计稿):无 intent-before-effect 崩溃恢复,
//! restore 时未完结 operation 归约为 aborted;单 lane;无 manual drive。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::agent::agent_loop::now_ms;
use crate::agent::harness::errors::OperationError;
use crate::agent::harness::events::{
    HarnessEvent, HarnessEventBus, MessageEvent, ToolEvent, ToolEventPhase, UsageEvent,
};
use crate::agent::harness::session::context::build_session_context;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    Entry, EntryOrder, EntryQuery, LaneRecord, ProvisionedEntry, ProvisionedMessageEntry,
    QueueKind, SessionTree, StepAttemptRecord, StepKind, ToolReplay, ToolStartedRecord,
    UsageCauseKind, UsageRecord,
};
use crate::agent::harness::types::{AgentHarnessStreamOptions, AgentHarnessTool, ToolContext};
use crate::agent::llm::SimpleStreamOptions;
use crate::agent::types::{
    AgentEvent, AgentListener, AgentMessage, AgentTool, QueueMode, TypedMessage,
};
use crate::agent::harness::uuid::uuid_v7;

/// 排队条目(entry_id 对应 `QueueEnqueuedRecord.target.id`,消费时物化为同 id 条目)。
#[derive(Clone, Debug)]
pub(crate) struct QueuedEntry {
    pub entry_id: String,
    pub message: AgentMessage,
}

/// 三条持久化队列的内存视图。
#[derive(Default)]
pub(crate) struct QueueSet {
    pub steer: Vec<QueuedEntry>,
    pub follow_up: Vec<QueuedEntry>,
    pub next_run: Vec<QueuedEntry>,
}

impl QueueSet {
    /// 按 drain 模式取出队列头部条目。
    pub fn drain(queue: &mut Vec<QueuedEntry>, mode: QueueMode) -> Vec<QueuedEntry> {
        match mode {
            QueueMode::All => std::mem::take(queue),
            QueueMode::OneAtATime => {
                if queue.is_empty() {
                    Vec::new()
                } else {
                    vec![queue.remove(0)]
                }
            }
        }
    }
}

/// 运行期引擎句柄(持 signal 供 abort;agent 供重试移除失败消息)。
pub(crate) struct EngineHandle {
    pub run_id: String,
    pub signal: tokio_util::sync::CancellationToken,
    pub agent: Arc<crate::agent::agent::Agent>,
}

/// 运行期共享依赖(存入 [`HarnessState`](super::agent_harness::HarnessState),
/// 克隆进闭包/任务)。
#[derive(Clone)]
pub(crate) struct RuntimeShared {
    pub session: Session,
    pub bus: HarnessEventBus,
    pub queues: Arc<Mutex<QueueSet>>,
    pub engine: Arc<Mutex<Option<EngineHandle>>>,
    pub busy: Arc<tokio::sync::watch::Sender<bool>>,
}

impl RuntimeShared {
    pub fn new(session: Session) -> Self {
        let (busy, _) = tokio::sync::watch::channel(false);
        Self {
            session,
            bus: HarnessEventBus::new(),
            queues: Arc::new(Mutex::new(QueueSet::default())),
            engine: Arc::new(Mutex::new(None)),
            busy: Arc::new(busy),
        }
    }
}

/// core `AgentTool` → harness 工具适配(忽略 context;内置 create_*_tool 构造器
/// 捕获 env,不依赖每回合 context)。
pub fn harness_tool_from_core(tool: AgentTool) -> AgentHarnessTool {
    let execute = tool.execute.clone();
    AgentHarnessTool {
        name: tool.name,
        label: tool.label,
        description: tool.description,
        parameters: tool.parameters,
        execution_mode: tool.execution_mode,
        prepare_arguments: tool.prepare_arguments,
        execute: Arc::new(
            move |tool_call_id: String,
                  params: serde_json::Value,
                  signal,
                  on_update,
                  _context: Arc<dyn ToolContext>| {
                let execute = execute.clone();
                Box::pin(async move { execute(tool_call_id, params, signal, on_update).await })
            },
        ),
    }
}

/// 空工具上下文(tool_context 未提供时的占位绑定目标)。
pub(crate) struct EmptyToolContext;

impl ToolContext for EmptyToolContext {}

/// harness 流选项 → provider 流选项(字段一一映射)。
pub(crate) fn stream_options_to_simple(options: &AgentHarnessStreamOptions) -> SimpleStreamOptions {
    SimpleStreamOptions {
        transport: options.transport.clone(),
        timeout_ms: options.timeout_ms,
        max_retries: options.max_retries,
        max_retry_delay_ms: options.max_retry_delay_ms,
        headers: options.headers.clone(),
        metadata: options.metadata.clone(),
        cache_retention: options.cache_retention,
        ..Default::default()
    }
}

/// 从 session 当前分支构建时间序上下文(消息 + 派生状态)。
pub(crate) async fn build_history(
    session: &Session,
) -> Result<crate::agent::harness::session::context::SessionContext, crate::agent::harness::session::types::SessionError>
{
    if session.get_leaf_id().await?.is_none() {
        return Ok(Default::default());
    }
    let entries = session
        .find_entries_on_branch(crate::agent::harness::session::types::BranchQuery {
            query: EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            },
            bounds: Default::default(),
        })
        .await?;
    Ok(build_session_context(
        &entries,
        &crate::agent::harness::session::context::SessionContextBuildOptions::default(),
    ))
}

/// 把分支条目(时间序)取出,供 compaction 准备复用。
pub(crate) async fn branch_entries(
    session: &Session,
) -> Result<Vec<Entry>, crate::agent::harness::session::types::SessionError> {
    if session.get_leaf_id().await?.is_none() {
        return Ok(Vec::new());
    }
    session
        .find_entries_on_branch(crate::agent::harness::session::types::BranchQuery {
            query: EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            },
            bounds: Default::default(),
        })
        .await
}

/// SessionError → OperationError(记录落库失败的统一包装)。
pub(crate) fn operation_error(error: crate::agent::harness::session::types::SessionError) -> OperationError {
    OperationError {
        code: format!("session_{}", error.code),
        message: error.message,
    }
}

/// 构造队列 drain 闭包:从 [`QueueSet`] 按 mode 取条目并回传给 loop 注入。
/// 不在此处落库——loop 会为注入消息发出 MessageStart/MessageEnd,由镜像监听器
/// 统一追加条目(否则同一条消息会双写)。
pub(crate) fn make_queue_getter(
    shared: RuntimeShared,
    run_id: String,
    queue_kind: QueueKind,
    mode: QueueMode,
) -> crate::agent::types::GetQueuedMessagesFn {
    Arc::new(move || {
        let shared = shared.clone();
        let run_id = run_id.clone();
        Box::pin(async move {
            let mut queues = shared
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let drained: Vec<QueuedEntry> = match queue_kind {
                QueueKind::Steer => QueueSet::drain(&mut queues.steer, mode),
                QueueKind::FollowUp => QueueSet::drain(&mut queues.follow_up, mode),
                QueueKind::NextRun => QueueSet::drain(&mut queues.next_run, mode),
            };
            let _ = run_id;
            drained.into_iter().map(|item| item.message).collect()
        })
    })
}

/// 镜像监听器:AgentEvent → session 条目/记录 + harness 事件。
pub(crate) fn make_mirroring_listener(shared: RuntimeShared, run_id: String) -> AgentListener {
    let attempt_counter = Arc::new(AtomicI64::new(0));
    let tool_counter = Arc::new(AtomicUsize::new(0));
    let last_assistant_entry: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // tool_call_id → (result entry id, index)
    let tool_registry: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));

    Arc::new(move |event, _signal| {
        let shared = shared.clone();
        let run_id = run_id.clone();
        let attempt_counter = attempt_counter.clone();
        let tool_counter = tool_counter.clone();
        let last_assistant_entry = last_assistant_entry.clone();
        let tool_registry = tool_registry.clone();
        Box::pin(async move {
            match event {
                AgentEvent::MessageEnd { message } => {
                    // 工具结果条目复用 ToolStarted 预注册的 result entry id。
                    let pre_registered = match &message {
                        AgentMessage::Message(TypedMessage::ToolResult(tool_result)) => {
                            tool_registry
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .remove(&tool_result.tool_call_id)
                        }
                        _ => None,
                    };
                    let entry_id = pre_registered.unwrap_or_else(uuid_v7);
                    let provisioned = ProvisionedEntry::Message(ProvisionedMessageEntry {
                        id: entry_id.clone(),
                        message: message.clone(),
                        terminate: None,
                    });
                    let Ok(entry) = shared
                        .session
                        .append_entry(provisioned, "main".to_string())
                        .await
                    else {
                        return;
                    };
                    let entry_id = entry.id().to_string();
                    shared.bus.emit(&HarnessEvent::Message(MessageEvent {
                        lane: "main".to_string(),
                        run_id: run_id.clone(),
                        entry_id: entry_id.clone(),
                        message: message.clone(),
                    }));
                    let AgentMessage::Message(TypedMessage::Assistant(assistant)) = &message else {
                        return;
                    };
                    *last_assistant_entry
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(entry_id.clone());
                    let attempt = attempt_counter.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ = shared
                        .session
                        .append_record(LaneRecord::StepAttempt(StepAttemptRecord {
                            id: uuid_v7(),
                            seq: 0,
                            lane: "main".to_string(),
                            timestamp: now_ms(),
                            run_id: run_id.clone(),
                            step: StepKind::Assistant,
                            attempt,
                            result_entry_id: entry_id.clone(),
                            compaction_reason: None,
                        }))
                        .await;
                    let usage = assistant.usage.clone();
                    let _ = shared
                        .session
                        .append_record(LaneRecord::Usage(UsageRecord {
                            id: uuid_v7(),
                            seq: 0,
                            lane: "main".to_string(),
                            timestamp: now_ms(),
                            usage,
                            cause: UsageCauseKind::Assistant,
                            run_id: Some(run_id.clone()),
                            entry_id: Some(entry_id.clone()),
                            attempt: Some(attempt),
                            stop_reason: Some(assistant.stop_reason.clone()),
                            tool_call_id: None,
                            details: None,
                        }))
                        .await;
                    shared.bus.emit(&HarnessEvent::Usage(UsageEvent {
                        lane: "main".to_string(),
                        run_id,
                        usage: assistant.usage.clone(),
                        entry_id: Some(entry_id),
                    }));
                }
                AgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    args,
                } => {
                    let index = tool_counter.fetch_add(1, Ordering::SeqCst);
                    let assistant_entry_id = last_assistant_entry
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                        .unwrap_or_default();
                    let result_entry_id = uuid_v7();
                    tool_registry
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(tool_call_id.clone(), result_entry_id.clone());
                    let _ = shared
                        .session
                        .append_record(LaneRecord::ToolStarted(ToolStartedRecord {
                            id: uuid_v7(),
                            seq: 0,
                            lane: "main".to_string(),
                            timestamp: now_ms(),
                            run_id: run_id.clone(),
                            assistant_entry_id,
                            tool_index: index,
                            tool_call_id: tool_call_id.clone(),
                            tool_name: tool_name.clone(),
                            effective_args: args.as_object().cloned().unwrap_or_default(),
                            result_entry_id,
                            replay: ToolReplay::Never,
                        }))
                        .await;
                    shared.bus.emit(&HarnessEvent::Tool(ToolEvent {
                        lane: "main".to_string(),
                        run_id,
                        phase: ToolEventPhase::Start,
                        tool_call_id,
                        tool_name,
                        args: Some(args),
                        partial_result: None,
                        result: None,
                        is_error: false,
                    }));
                }
                AgentEvent::ToolExecutionUpdate {
                    tool_call_id,
                    tool_name,
                    args,
                    partial_result,
                } => {
                    shared.bus.emit(&HarnessEvent::Tool(ToolEvent {
                        lane: "main".to_string(),
                        run_id,
                        phase: ToolEventPhase::Update,
                        tool_call_id,
                        tool_name,
                        args: Some(args),
                        partial_result: Some(partial_result),
                        result: None,
                        is_error: false,
                    }));
                }
                AgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    tool_name,
                    result,
                    is_error,
                } => {
                    shared.bus.emit(&HarnessEvent::Tool(ToolEvent {
                        lane: "main".to_string(),
                        run_id,
                        phase: ToolEventPhase::End,
                        tool_call_id,
                        tool_name,
                        args: None,
                        partial_result: None,
                        result: Some(result),
                        is_error,
                    }));
                }
                // 工具结果已在 MessageEnd 落库;AgentStart/TurnStart/MessageStart/
                // MessageUpdate/AgentEnd 无落库语义。
                AgentEvent::AgentStart
                | AgentEvent::AgentEnd { .. }
                | AgentEvent::TurnStart
                | AgentEvent::TurnEnd { .. }
                | AgentEvent::MessageStart { .. }
                | AgentEvent::MessageUpdate { .. } => {}
            }
        })
    })
}

// ---------------------------------------------------------------------------
// 运行时集成测试(脚本化 stream_fn;blueprint 见 agent_loop::testing)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::agent::agent_loop::testing::{
        error_script, scripted_stream_fn, test_assistant, test_model, text_script,
        tool_call_script, user_message,
    };
    use crate::agent::harness::agent_harness::{
        AgentHarness, AgentHarnessOptions, OperationKind, QueueInput, RetryPolicy, RunOutcome,
        RunRejected, SuspensionReason,
    };
    use crate::agent::harness::session::memory::InMemorySessionStorage;
    use crate::agent::harness::session::types::{
        EntryOrder, OperationIntent, OperationOutcome, OperationStartedRecord, RecordQuery,
        SessionMetadata,
    };
    use crate::agent::llm::types::{AssistantContent, ModelThinkingLevel, StopReason, ToolCall};
    use crate::agent::types::{AgentState, AgentTool, AgentToolResult, ToolExecutionMode};
    use std::collections::HashMap as StdHashMap;
    use std::time::Duration;

    fn memory_session() -> Session {
        Session::new(Arc::new(InMemorySessionStorage::new(SessionMetadata {
            id: uuid_v7(),
            created_at: now_ms(),
            parent_session_id: None,
        })))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn options(
        session: Session,
        stream_fn: crate::agent::types::StreamFn,
        tools: Vec<AgentHarnessTool>,
        retry: Option<RetryPolicy>,
    ) -> AgentHarnessOptions {
        AgentHarnessOptions {
            session,
            stream_fn,
            model: test_model(),
            thinking_level: Some(ModelThinkingLevel::Off),
            active_tool_names: None,
            tools,
            tool_context: None,
            system_prompt: Some("test-system".to_string()),
            resources: Default::default(),
            stream_options: Default::default(),
            retry,
            compaction: None,
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            tool_execution: ToolExecutionMode::Parallel,
            telemetry_context: None,
        }
    }

    fn noop_tool(name: &str) -> AgentTool {
        AgentTool {
            name: name.to_string(),
            label: name.to_string(),
            description: "noop".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            execution_mode: None,
            prepare_arguments: None,
            execute: Arc::new(|_id, _params, _signal, _update| {
                Box::pin(async { Ok(AgentToolResult::text("ok")) })
            }),
        }
    }

    async fn records_of(session: &Session, record_type: &str) -> Vec<LaneRecord> {
        session
            .find_records(RecordQuery {
                record_type: Some(record_type.to_string()),
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            })
            .await
            .unwrap()
    }

    async fn message_roles(session: &Session) -> Vec<&'static str> {
        session
            .find_entries(EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            })
            .await
            .unwrap()
            .iter()
            .filter_map(|entry| match entry {
                Entry::Message(message) => match &message.message {
                    AgentMessage::Message(TypedMessage::User(_)) => Some("user"),
                    AgentMessage::Message(TypedMessage::Assistant(_)) => Some("assistant"),
                    AgentMessage::Message(TypedMessage::ToolResult(_)) => Some("toolResult"),
                    AgentMessage::Custom(_) => Some("custom"),
                },
                _ => None,
            })
            .collect()
    }

    async fn wait_for_operation(harness: &AgentHarness) {
        for _ in 0..300 {
            if harness
                .lane("main".to_string())
                .await
                .unwrap()
                .and_then(|lane| lane.operation)
                .is_some()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("operation never started");
    }

    /// 门控 + context 捕获的脚本流:每次调用等待 gate,并把 LLM Context 克隆存档。
    fn gated_capturing_stream_fn(
        gate: Arc<tokio::sync::Notify>,
    ) -> (
        crate::agent::types::StreamFn,
        Arc<Mutex<Vec<crate::agent::llm::Context>>>,
    ) {
        let calls: Arc<Mutex<Vec<crate::agent::llm::Context>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = calls.clone();
        let stream_fn: crate::agent::types::StreamFn = Arc::new(move |model, context, options| {
            let gate = gate.clone();
            let sink = sink.clone();
            Box::pin(async move {
                sink.lock().unwrap().push(context.clone());
                gate.notified().await;
                let _ = (model, options);
                let (stream, writer) = crate::agent::llm::event_stream::event_stream();
                let script = text_script("ok");
                for event in script.events {
                    writer.push(event);
                }
                writer.end(script.result);
                stream
            })
        });
        (stream_fn, calls)
    }

    fn gated_stream_fn(gate: Arc<tokio::sync::Notify>) -> crate::agent::types::StreamFn {
        Arc::new(move |_model, _context, _options| {
            let gate = gate.clone();
            Box::pin(async move {
                gate.notified().await;
                let (stream, writer) = crate::agent::llm::event_stream::event_stream();
                let script = text_script("slow");
                for event in script.events {
                    writer.push(event);
                }
                writer.end(script.result);
                stream
            })
        })
    }

    #[tokio::test]
    async fn prompt_completes_with_session_trail_and_events() {
        let session = memory_session();
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("hello world")]);
        let (harness, suspended) =
            AgentHarness::create(options(session.clone(), stream_fn, Vec::new(), None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        assert!(suspended.is_empty());

        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        for event_type in [
            crate::agent::harness::events::HarnessEventType::RunStart,
            crate::agent::harness::events::HarnessEventType::RunEnd,
            crate::agent::harness::events::HarnessEventType::Message,
            crate::agent::harness::events::HarnessEventType::Tool,
            crate::agent::harness::events::HarnessEventType::Usage,
        ] {
            let seen = seen.clone();
            harness
                .events()
                .on(event_type, Arc::new(move |event| {
                    seen.lock()
                        .unwrap()
                        .push(event.event_type().as_str().to_string());
                }));
        }

        let outcome = harness.prompt("hi".to_string()).await.unwrap();
        let RunOutcome::Completed { final_message, .. } = &outcome else {
            panic!("expected completed");
        };
        assert_eq!(final_message.content.len(), 1);
        harness.wait_for_idle().await.unwrap();

        // 事件:run_start + 若干 message/usage + run_end。
        let events = seen.lock().unwrap().clone();
        assert_eq!(events.first().unwrap(), "run_start");
        assert_eq!(events.last().unwrap(), "run_end");
        assert!(events.contains(&"message".to_string()));
        assert!(events.contains(&"usage".to_string()));

        // 条目:user + assistant。
        assert_eq!(message_roles(&session).await, vec!["user", "assistant"]);

        // 记录:operation_started/finished、step_attempt、usage。
        assert_eq!(records_of(&session, "operation_started").await.len(), 1);
        let finished = records_of(&session, "operation_finished").await;
        assert!(matches!(
            &finished[0],
            LaneRecord::OperationFinished(record)
                if record.outcome == OperationOutcome::Completed
        ));
        let attempts = records_of(&session, "step_attempt").await;
        assert!(matches!(
            &attempts[0],
            LaneRecord::StepAttempt(record)
                if record.attempt == 1 && record.step == StepKind::Assistant
        ));
        let usage = records_of(&session, "usage").await;
        assert!(matches!(
            &usage[0],
            LaneRecord::Usage(record) if record.cause == UsageCauseKind::Assistant
        ));
    }

    #[tokio::test]
    async fn busy_prompt_rejected_until_idle() {
        let session = memory_session();
        let gate = Arc::new(tokio::sync::Notify::new());
        let (harness, _) =
            AgentHarness::create(options(session, gated_stream_fn(gate.clone()), Vec::new(), None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        let run = {
            let harness = harness.clone();
            tokio::spawn(async move { harness.prompt("hi".to_string()).await })
        };
        wait_for_operation(&harness).await;
        // 忙时第二次 prompt 被 LaneBusy 拒绝。
        assert!(matches!(
            harness.prompt("second".to_string()).await,
            Err(RunRejected::LaneBusy(_))
        ));
        gate.notify_one();
        let outcome = run.await.unwrap().unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));
        harness.wait_for_idle().await.unwrap();
        assert!(harness
            .lane("main".to_string())
            .await
            .unwrap()
            .unwrap()
            .operation
            .is_none());
    }

    #[tokio::test]
    async fn abort_mid_run_marks_aborted_and_returns_queues() {
        let session = memory_session();
        let gate = Arc::new(tokio::sync::Notify::new());
        let (harness, _) = AgentHarness::create(
            options(session.clone(), gated_stream_fn(gate.clone()), Vec::new(), None),
        )
        .await
        .unwrap();
        let harness = Arc::new(harness);
        let run = {
            let harness = harness.clone();
            tokio::spawn(async move { harness.prompt("hi".to_string()).await })
        };
        wait_for_operation(&harness).await;
        // 忙时入队 steer;abort 后载荷随 AbortOutcome 返回。
        let steer_id = harness
            .steer(QueueInput::Text("queued".to_string()))
            .await
            .unwrap();
        let aborted = harness.abort().await.unwrap();
        assert_eq!(aborted.steer.len(), 1);
        gate.notify_one();
        let outcome = run.await.unwrap().unwrap();
        assert!(matches!(outcome, RunOutcome::Aborted { .. }));

        let finished = records_of(&session, "operation_finished").await;
        assert!(matches!(
            &finished[0],
            LaneRecord::OperationFinished(record)
                if record.outcome == OperationOutcome::Aborted
        ));
        assert_eq!(records_of(&session, "abort_requested").await.len(), 1);
        // 已随 abort 清空的 steer 条目再次取消 → 按 enqueued 记录 + 运行已收尾
        // 判定为 cleared。
        let cancel = harness.cancel_queued(steer_id).await.unwrap();
        assert_eq!(
            cancel,
            crate::agent::harness::agent_harness::CancelQueuedOutcome::AlreadyCleared
        );
    }

    #[tokio::test]
    async fn steer_consumed_between_turns_and_persisted() {
        let session = memory_session();
        // 每个 stream 调用由独立 gate 控制,保证 steer 在回合间入队。
        let gates: Arc<Mutex<Vec<Arc<tokio::sync::Notify>>>> = Arc::new(Mutex::new(vec![
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(tokio::sync::Notify::new()),
        ]));
        let call_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stream_fn: crate::agent::types::StreamFn = {
            let gates = gates.clone();
            let call_counter = call_counter.clone();
            Arc::new(move |_model, _context, _options| {
                let gates = gates.clone();
                let call_counter = call_counter.clone();
                Box::pin(async move {
                    let index = call_counter.fetch_add(1, Ordering::SeqCst);
                    let gate = gates.lock().unwrap()[index].clone();
                    gate.notified().await;
                    let (stream, writer) = crate::agent::llm::event_stream::event_stream();
                    let script = if index == 0 { text_script("one") } else { text_script("two") };
                    for event in script.events {
                        writer.push(event);
                    }
                    writer.end(script.result);
                    stream
                })
            })
        };
        let (stream_fn, calls) = {
            // 复用 scripted 捕获:包一层记录 context。
            let inner = stream_fn;
            let calls = Arc::new(Mutex::new(Vec::new()));
            let sink = calls.clone();
            let wrapped: crate::agent::types::StreamFn = Arc::new(move |model, context, options| {
                let inner = inner.clone();
                let sink = sink.clone();
                Box::pin(async move {
                    sink.lock().unwrap().push(context.clone());
                    inner(model, context, options).await
                })
            });
            (wrapped, calls)
        };
        let (harness, _) =
            AgentHarness::create(options(session.clone(), stream_fn, Vec::new(), None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        let run = {
            let harness = harness.clone();
            tokio::spawn(async move { harness.prompt("start".to_string()).await })
        };
        wait_for_operation(&harness).await;
        harness
            .steer(QueueInput::Text("inject".to_string()))
            .await
            .unwrap();
        // 放行第一回合;loop 在回合边界 drain steering 并发起第二次调用。
        gates.lock().unwrap()[0].notify_one();
        for _ in 0..300 {
            if call_counter.load(Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            call_counter.load(Ordering::SeqCst),
            2,
            "steering must trigger the second turn"
        );
        gates.lock().unwrap()[1].notify_one();
        let outcome = run.await.unwrap().unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));

        // 第二次 LLM 调用的 context 末尾应是被注入的 user 消息。
        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let last = captured[1].messages.last().unwrap();
        assert!(
            matches!(last, crate::agent::llm::types::Message::User(_)),
            "expected trailing user message, got {last:?}"
        );

        assert_eq!(
            message_roles(&session).await,
            vec!["user", "assistant", "user", "assistant"]
        );
    }

    #[tokio::test]
    async fn next_run_materialized_in_following_prompt() {
        let session = memory_session();
        let gate = Arc::new(tokio::sync::Notify::new());
        let (stream_fn, calls) = gated_capturing_stream_fn(gate.clone());
        let (harness, _) =
            AgentHarness::create(options(session.clone(), stream_fn, Vec::new(), None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        // 空闲时队列方法要求活跃 run。
        assert!(harness
            .next_run(QueueInput::Text("cached".to_string()))
            .await
            .is_err());

        let run = {
            let harness = harness.clone();
            tokio::spawn(async move { harness.prompt("go".to_string()).await })
        };
        wait_for_operation(&harness).await;
        // 忙时入队 nextRun;run 2 开始时前置于新 prompt 之前。
        harness
            .next_run(QueueInput::Text("cached".to_string()))
            .await
            .unwrap();
        gate.notify_one();
        let outcome = run.await.unwrap().unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));

        // run 2 的流调用同样被 gate 门控,需再次放行。
        let run2 = {
            let harness = harness.clone();
            tokio::spawn(async move { harness.prompt("again".to_string()).await })
        };
        gate.notify_one();
        let outcome = run2.await.unwrap().unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));

        // 第二次 run 的 LLM context:go/first + cached(nextRun 前置) + again。
        let captured = calls.lock().unwrap();
        assert_eq!(captured.len(), 2);
        let history: Vec<&str> = captured[1]
            .messages
            .iter()
            .map(|message| match message {
                crate::agent::llm::types::Message::User(_) => "user",
                crate::agent::llm::types::Message::Assistant(_) => "assistant",
                crate::agent::llm::types::Message::ToolResult(_) => "toolResult",
            })
            .collect();
        assert_eq!(history, vec!["user", "assistant", "user", "user"], "{history:?}");
    }

    #[tokio::test]
    async fn retryable_error_retried_until_success() {
        let session = memory_session();
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            error_script(StopReason::Error, "429 too many requests"),
            text_script("recovered"),
        ]);
        let retry = RetryPolicy {
            enabled: true,
            max_retries: 2,
            base_delay_ms: 1,
        };
        let (harness, _) =
            AgentHarness::create(options(session.clone(), stream_fn, Vec::new(), Some(retry)))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        let outcome = harness.prompt("hi".to_string()).await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));

        // 失败 attempt 保留在 session 历史:用户 + 失败 assistant + 成功 assistant。
        assert_eq!(
            message_roles(&session).await,
            vec!["user", "assistant", "assistant"]
        );
        // 两次 LLM attempt 各一条 step_attempt。
        assert_eq!(records_of(&session, "step_attempt").await.len(), 2);
    }

    #[tokio::test]
    async fn tool_loop_persists_tool_records_and_results() {
        let session = memory_session();
        let tool_call = ToolCall {
            id: "call-1".to_string(),
            name: "noop".to_string(),
            arguments: serde_json::from_value(serde_json::json!({})).unwrap(),
            thought_signature: None,
            namespace: None,
        };
        let (stream_fn, _calls) = scripted_stream_fn(vec![
            tool_call_script(vec![tool_call], "calling"),
            text_script("done"),
        ]);
        let tool = harness_tool_from_core(noop_tool("noop"));
        let (harness, _) =
            AgentHarness::create(options(session.clone(), stream_fn, vec![tool], None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        let outcome = harness.prompt("use tool".to_string()).await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));

        let started = records_of(&session, "tool_started").await;
        assert!(matches!(
            &started[0],
            LaneRecord::ToolStarted(record) if record.tool_name == "noop"
        ));
        assert_eq!(
            message_roles(&session).await,
            vec!["user", "assistant", "toolResult", "assistant"]
        );
    }

    #[tokio::test]
    async fn restore_reduces_open_operation_to_aborted() {
        let session = memory_session();
        // 模拟崩溃残留:无 operation_finished 的 operation_started。
        session
            .append_record(LaneRecord::OperationStarted(OperationStartedRecord {
                id: "crashed-run".to_string(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                source_leaf_id: None,
                intent: OperationIntent::Run {
                    original_prompt: vec![user_message("lost", 1)],
                    initial_messages: Vec::new(),
                    system_prompt_override: None,
                    resume_data: None,
                },
            }))
            .await
            .unwrap();

        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("fresh")]);
        let (harness, suspended) =
            AgentHarness::create(options(session.clone(), stream_fn, Vec::new(), None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        assert_eq!(suspended.len(), 1);
        assert_eq!(suspended[0].id, "crashed-run");
        assert_eq!(suspended[0].reason, SuspensionReason::Crash);
        assert!(matches!(suspended[0].kind, OperationKind::Run));
        // resume 无挂起可续。
        assert!(matches!(
            harness.resume().await,
            Err(crate::agent::harness::agent_harness::ResumeRejected::NothingToResume(_))
        ));
        // 残留 operation 已归约 aborted,新 run 可正常进行。
        let outcome = harness.prompt("fresh".to_string()).await.unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { .. }));
    }

    #[tokio::test]
    async fn record_usage_and_stream_options_patch() {
        let session = memory_session();
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("ok")]);
        let (harness, _) = AgentHarness::create(options(session, stream_fn, Vec::new(), None))
            .await
            .unwrap();
        harness
            .record_usage(crate::agent::llm::types::Usage::default(), None)
            .await
            .unwrap();
        let usage = records_of(harness.session(), "usage").await;
        assert!(matches!(
            &usage[0],
            LaneRecord::Usage(record) if record.cause == UsageCauseKind::Adjustment
        ));

        harness
            .patch_stream_options(crate::agent::harness::types::AgentHarnessStreamOptionsPatch {
                timeout_ms: Some(1_000),
                headers: Some(Some(StdHashMap::from([(
                    "x-a".to_string(),
                    Some("1".to_string()),
                )]))),
                ..Default::default()
            })
            .await;
        let stream_options = harness.get_stream_options().await;
        assert_eq!(stream_options.timeout_ms, Some(1_000));
        assert_eq!(stream_options.headers.as_ref().unwrap()["x-a"], "1");
        // 删除键 + 标量回退。
        harness
            .patch_stream_options(crate::agent::harness::types::AgentHarnessStreamOptionsPatch {
                headers: Some(Some(StdHashMap::from([("x-a".to_string(), None)]))),
                timeout_ms: Some(2_000),
                ..Default::default()
            })
            .await;
        let stream_options = harness.get_stream_options().await;
        assert!(stream_options.headers.unwrap().is_empty());
        assert_eq!(stream_options.timeout_ms, Some(2_000));
    }

    #[tokio::test]
    async fn compact_manual_appends_compaction_entry() {
        let session = memory_session();
        session.append_message(user_message("q1", 1)).await.unwrap();
        session
            .append_message(AgentMessage::Message(TypedMessage::Assistant(test_assistant(
                vec![AssistantContent::text("a1")],
                StopReason::Stop,
            ))))
            .await
            .unwrap();
        let (stream_fn, _calls) = scripted_stream_fn(vec![text_script("summary of history")]);
        let (harness, _) =
            AgentHarness::create(options(session.clone(), stream_fn, Vec::new(), None))
                .await
                .unwrap();

        let outcome = harness.compact(None).await.unwrap();
        let crate::agent::harness::agent_harness::CompactionOutcome::Completed { entry, .. } =
            outcome
        else {
            panic!("expected completed compaction");
        };
        assert!(entry.summary.contains("summary of history"), "{}", entry.summary);

        let finished = records_of(&session, "operation_finished").await;
        assert!(matches!(
            &finished[0],
            LaneRecord::OperationFinished(record)
                if record.outcome == OperationOutcome::Completed
        ));
        let attempts = records_of(&session, "step_attempt").await;
        assert!(matches!(
            &attempts[0],
            LaneRecord::StepAttempt(record) if record.step == StepKind::Compaction
        ));
    }

    #[tokio::test]
    async fn close_rejects_and_cancels_active_run() {
        let session = memory_session();
        let gate = Arc::new(tokio::sync::Notify::new());
        let (harness, _) =
            AgentHarness::create(options(session, gated_stream_fn(gate.clone()), Vec::new(), None))
                .await
                .unwrap();
        let harness = Arc::new(harness);
        let run = {
            let harness = harness.clone();
            tokio::spawn(async move { harness.prompt("hi".to_string()).await })
        };
        wait_for_operation(&harness).await;
        harness.close().await;
        // 关闭后运行方法返回 Closed。
        assert!(matches!(
            harness.prompt("x".to_string()).await,
            Err(RunRejected::Closed(_))
        ));
        gate.notify_one();
        // 在途 run 被取消信号收尾(关键是有限时间内结束)。
        let _ = tokio::time::timeout(Duration::from_secs(10), run)
            .await
            .expect("run must finish after close");
    }
}
