//! prompt 运行管线(自 `agent_harness.rs` 拆出):`prompt`/`prompt_input` 主流程。
//!
//! `prompt_input` 按阶段分解,阶段边界:
//! 1. [`AgentHarness::prepare_prompt`]:守卫 + 配置快照 → 输入归一化 +
//!    BeforeRun hook → nextRun 捕获 + durable intent 落库 → 前置条目物化;
//! 2. [`AgentHarness::build_run_context`]:历史构建 → pre-prompt 阈值压缩 →
//!    transform_context hook;
//! 3. [`AgentHarness::assemble_engine`]:loop 配置 + hook 闭包接线 → 创建引擎、
//!    注册引擎槽位、置 busy、emit RunStart;
//! 4. [`AgentHarness::drive_run`]:before_drive → 运行 → retry_loop(溢出恢复 +
//!    瞬态重试)→ post_run_compaction → before_run_end_loop(follow-up 续跑);
//! 5. [`AgentHarness::finalize_run`]:结果归约 + 收尾。
//!
//! 阶段 4 的失败出口统一经 [`AgentHarness::fail_run`](finish_run → RunEnd(Failed),
//! 顺序与拆分前一致);阶段 1/2 的提前返回经 `Err(RunResult)` 通道原样上抛。

use crate::agent::agent::Agent;
use crate::agent::agent_loop::now_ms;
use crate::agent::harness::agent_harness::{
    AgentHarness, QueueInput, RetryPolicy, RunOutcome, RunRejected, RunResult,
};
use crate::agent::harness::compaction::compaction as compaction_mod;
use crate::agent::harness::compaction_runner::{
    guarded_context_tokens, run_auto_compaction, session_latest_compaction_timestamp,
};
use crate::agent::harness::context::Context as HarnessContext;
use crate::agent::harness::errors::{InvalidMessage, OperationError};
use crate::agent::harness::events::{HarnessEvent, RunEndEvent, RunEndOutcome, RunStartEvent};
use crate::agent::harness::execution::{create_gate, Gate, GateControl};
use crate::agent::harness::hooks::{
    AfterToolEvent, BeforeDriveEvent, BeforePayloadEvent, BeforeRequestEvent, BeforeRunEndEvent,
    BeforeRunEvent, BeforeToolEvent, DriveOperation, HookEvent, HookName, HookRegistry, HookResult,
    RequestStep, TransformContextEvent,
};
use crate::agent::harness::runtime::{
    build_history, make_mirroring_listener, make_queue_getter, operation_error,
    stream_options_to_simple, EmptyToolContext, EngineHandle, QueueSet, QueuedEntry, RuntimeShared,
};
use crate::agent::harness::session::types::{
    CompactionReason, LaneRecord, OperationIntent, OperationOutcome, OperationStartedRecord,
    ProvisionedEntry, ProvisionedMessageEntry, QueueKind, SessionTree,
};
use crate::agent::harness::telemetry::TelemetryContext;
use crate::agent::harness::types::{AgentHarnessResources, AgentHarnessStreamOptions, ToolContext};
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::overflow::{is_context_overflow, is_recoverable_length};
use crate::agent::llm::retry::{is_retryable_assistant_error, retry_delay_ms, sleep_with_cancel};
use crate::agent::llm::types::{
    AssistantMessage, Model, ModelThinkingLevel, OnPayloadFn, StopReason, Usage,
};
use crate::agent::types::{
    AfterToolCallHookFn, AfterToolCallResult, AgentContext, AgentLoopConfig, AgentLoopTurnUpdate,
    AgentMessage, AgentState, BeforeToolCallHookFn, BeforeToolCallResult, PrepareNextTurnContext,
    PrepareNextTurnFn, QueueMode, StreamFn, ToolExecutionMode, TypedMessage,
};
use std::sync::Arc;

/// prompt 启动时的配置快照(短临界区内克隆,不跨 await 持锁)。
pub(crate) struct PromptSnapshot {
    pub model: Model,
    pub thinking_level: ModelThinkingLevel,
    pub active_tool_names: Vec<String>,
    pub tools: Vec<crate::agent::harness::types::AgentHarnessTool>,
    pub tool_context: Option<Arc<dyn ToolContext>>,
    pub system_prompt: Option<String>,
    pub tool_execution: ToolExecutionMode,
    pub stream_options: AgentHarnessStreamOptions,
    pub retry_policy: RetryPolicy,
    pub stream_fn: StreamFn,
    pub resources: AgentHarnessResources,
    pub hooks: Arc<HookRegistry>,
    pub telemetry_context: Option<Arc<dyn TelemetryContext>>,
}

/// prepare_prompt 阶段产物:一次 run 的句柄/上下文与归一化输入。
struct PromptRunPrep {
    snapshot: PromptSnapshot,
    shared: RuntimeShared,
    run_id: String,
    hook_gate: Gate,
    hook_gate_control: GateControl,
    hook_context: HarnessContext,
    prompt_messages: Vec<AgentMessage>,
}

/// assemble_engine 阶段产物:已注册引擎槽位、就绪可驱动的运行引擎。
struct RunEngine {
    agent: Arc<Agent>,
    listener_id: u64,
    signal: tokio_util::sync::CancellationToken,
}

impl AgentHarness {
    pub async fn prompt(&self, text: String) -> RunResult {
        self.prompt_input(QueueInput::Text(text)).await
    }

    /// prompt 主流程:准备 → 上下文构造 → 引擎组装 → 运行(重试/压缩) → 收尾
    /// (阶段职责见模块注释)。各阶段返回 `Err(RunResult)` 表示按原语义提前
    /// 返回(守卫拒绝,或失败已经 fail_run/Failed 归约收尾)。
    async fn prompt_input(&self, input: QueueInput) -> RunResult {
        let mut prep = match self.prepare_prompt(input).await {
            Ok(prep) => prep,
            Err(result) => return result,
        };
        let history = match self.build_run_context(&mut prep).await {
            Ok(history) => history,
            Err(result) => return result,
        };
        let engine = self.assemble_engine(&mut prep, history).await;
        if let Some(result) = self.drive_run(&prep, &engine).await {
            return result;
        }
        self.finalize_run(&prep, engine).await
    }

    /// 阶段 1-4:守卫 + 配置快照 → 输入归一化 + BeforeRun hook → nextRun 捕获
    /// + durable intent 落库 → 前置条目物化。
    async fn prepare_prompt(&self, input: QueueInput) -> Result<PromptRunPrep, RunResult> {
        // 1. 守卫 + 配置快照(短临界区,不跨 await)。
        let snapshot = match self.guard_and_snapshot() {
            Ok(snapshot) => snapshot,
            Err(rejected) => return Err(Err(rejected)),
        };
        // 2. 输入归一化。
        let mut prompt_messages: Vec<AgentMessage> = match input {
            QueueInput::Text(text) => vec![AgentMessage::user_text(text, now_ms())],
            QueueInput::Message(message) => vec![*message],
            QueueInput::Messages(messages) => messages,
        };
        if prompt_messages.is_empty() {
            return Err(Err(RunRejected::InvalidMessage(InvalidMessage::new(
                "Prompt must contain at least one message",
                "main".to_string(),
                "empty prompt".to_string(),
            ))));
        }

        let shared = self.shared();
        let run_id = uuid_v7();
        let (hook_gate, hook_gate_control) = create_gate();
        let hook_context = HarnessContext::new()
            .with_telemetry_opt(snapshot.telemetry_context.clone())
            .with_signal(hook_gate.signal());
        if let Some(HookResult::BeforeRun(Some(result))) = snapshot
            .hooks
            .run_with_gate(
                HookName::BeforeRun,
                HookEvent::BeforeRun(BeforeRunEvent {
                    lane: "main".to_string(),
                    run_id: run_id.clone(),
                    prompt: prompt_messages.clone(),
                    resources: resources_json(&snapshot.resources),
                }),
                &hook_gate,
                hook_context.clone(),
            )
            .await
            .ok()
            .flatten()
        {
            if let Some(messages) = result.messages {
                prompt_messages.extend(messages);
            }
        }

        // 3. nextRun 捕获项 + durable intent 落库。
        let initial: Vec<QueuedEntry> = {
            let mut queues = shared
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            QueueSet::drain(&mut queues.next_run, QueueMode::All)
        };
        let accept = self
            .session()
            .append_record(LaneRecord::OperationStarted(OperationStartedRecord {
                id: run_id.clone(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                source_leaf_id: self.session().get_leaf_id().await.unwrap_or(None),
                intent: OperationIntent::Run {
                    original_prompt: prompt_messages.clone(),
                    initial_messages: initial
                        .iter()
                        .map(|item| {
                            ProvisionedEntry::Message(ProvisionedMessageEntry {
                                id: item.entry_id.clone(),
                                message: item.message.clone(),
                                terminate: None,
                            })
                        })
                        .collect(),
                    system_prompt_override: None,
                    resume_data: None,
                },
            }))
            .await;
        if let Err(error) = accept {
            return Err(Ok(RunOutcome::Failed {
                leaf_id: self.leaf_id().await,
                error: operation_error(error),
                final_entry_id: None,
                final_message: None,
            }));
        }

        // 4. nextRun 前置条目物化。
        for item in &initial {
            let _ = self
                .session()
                .append_entry(
                    ProvisionedEntry::Message(ProvisionedMessageEntry {
                        id: item.entry_id.clone(),
                        message: item.message.clone(),
                        terminate: None,
                    }),
                    "main".to_string(),
                )
                .await;
        }

        Ok(PromptRunPrep {
            snapshot,
            shared,
            run_id,
            hook_gate,
            hook_gate_control,
            hook_context,
            prompt_messages,
        })
    }

    /// 阶段 5:历史构建(不含本次 prompt) → 5.5 pre-prompt 阈值压缩(对齐 pi
    /// `prompt()` 前置 `_checkCompaction`) → 5.8 transform_context hook。
    async fn build_run_context(
        &self,
        prep: &mut PromptRunPrep,
    ) -> Result<Vec<AgentMessage>, RunResult> {
        let snapshot = &mut prep.snapshot;
        let hook_gate = prep.hook_gate.clone();
        let hook_context = prep.hook_context.clone();
        let run_id = prep.run_id.clone();
        // 5. 历史(不含本次 prompt;prompt 由引擎经事件循环落库)。
        let mut history = match build_history(self.session()).await {
            Ok(history) => history.messages,
            Err(error) => {
                return Err(Ok(RunOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error: operation_error(error),
                    final_entry_id: None,
                    final_message: None,
                }));
            }
        };

        // 5.5 pre-prompt 阈值压缩(对齐 pi `prompt()` 前置 `_checkCompaction`):
        // 历史已越阈值时先压缩再组引擎。
        let compaction_settings = self.get_compaction_settings().await;
        if compaction_settings.enabled && snapshot.model.context_window > 0 {
            let tokens = guarded_context_tokens(
                &history,
                session_latest_compaction_timestamp(self.session()).await,
            );
            if tokens > 0
                && compaction_mod::should_compact(
                    tokens,
                    snapshot.model.context_window,
                    &compaction_settings,
                )
            {
                self.auto_compact(
                    CompactionReason::Threshold,
                    &hook_gate,
                    &hook_context,
                    &run_id,
                )
                .await;
                // 压缩后重建历史;失败则沿用旧历史继续(不阻断本次 run)
                if let Ok(rebuilt) = build_history(self.session()).await {
                    history = rebuilt.messages;
                }
            }
        }

        // 5.8 transform_context hook(初始 LLM context 组装前)。
        if let Some(HookResult::TransformContext(Some(result))) = snapshot
            .hooks
            .run_with_gate(
                HookName::TransformContext,
                HookEvent::TransformContext(TransformContextEvent {
                    lane: "main".to_string(),
                    run_id: run_id.clone(),
                    messages: history.clone(),
                    system_prompt: snapshot.system_prompt.clone().unwrap_or_default(),
                }),
                &hook_gate,
                hook_context.clone(),
            )
            .await
            .ok()
            .flatten()
        {
            if let Some(messages) = result.messages {
                history = messages;
            }
            if let Some(system_prompt) = result.system_prompt {
                snapshot.system_prompt = Some(system_prompt);
            }
        }

        Ok(history)
    }

    /// 阶段 6:组装引擎 Agent(loop 配置 + hook 闭包接线),注册引擎槽位、
    /// 置 busy 并 emit RunStart。
    async fn assemble_engine(
        &self,
        prep: &mut PromptRunPrep,
        history: Vec<AgentMessage>,
    ) -> RunEngine {
        let snapshot = &mut prep.snapshot;
        let shared = prep.shared.clone();
        let run_id = prep.run_id.clone();
        let hook_gate = prep.hook_gate.clone();
        let hook_context = prep.hook_context.clone();
        let hook_gate_control = prep.hook_gate_control.clone();
        // 6. 组装引擎 Agent。
        let (steering_mode, follow_up_mode) = (
            self.get_steering_mode().await,
            self.get_follow_up_mode().await,
        );
        let active: std::collections::HashSet<String> =
            snapshot.active_tool_names.iter().cloned().collect();
        let context_source = crate::agent::harness::types::AgentHarnessToolContextSource::Static(
            snapshot
                .tool_context
                .clone()
                .unwrap_or_else(|| Arc::new(EmptyToolContext)),
        );
        let tools: Vec<crate::agent::types::AgentTool> = snapshot
            .tools
            .iter()
            .filter(|tool| active.contains(&tool.name))
            .map(|tool| {
                crate::agent::harness::types::bind_harness_tool(
                    tool.clone(),
                    context_source.clone(),
                )
            })
            .collect();
        // mid-run 阈值压缩(对齐 pi `_compactBeforeNextAssistantResponse`):
        // 回合间隙估算上下文,越阈值则压缩会话并用压缩后的历史替换下一回合上下文。
        let prepare_next_turn: Option<PrepareNextTurnFn> = self
            .prepare_next_turn_hook(snapshot, &hook_gate, &hook_context, &run_id)
            .await;
        // 6.2 before_request hook(首次 assistant 请求前)。
        snapshot.stream_options =
            resolve_hook_stream_options(snapshot, &hook_gate, &hook_context, &run_id).await;

        let mut simple_stream_options = stream_options_to_simple(&snapshot.stream_options);
        simple_stream_options.on_payload =
            Some(payload_hook(snapshot, &hook_gate, &hook_context, &run_id));

        let loop_config = AgentLoopConfig {
            model: snapshot.model.clone(),
            stream: simple_stream_options,
            // harness 版转换:识别 compactionSummary/branchSummary 等自定义消息
            // (core 版会丢弃,压缩摘要将不进上下文)。
            convert_to_llm: std::sync::Arc::new(|messages| {
                Box::pin(async move { crate::agent::harness::messages::convert_to_llm(messages) })
            }),
            transform_context: None,
            get_api_key: None,
            finish_turn: None,
            prepare_next_turn,
            get_steering_messages: Some(make_queue_getter(
                shared.clone(),
                run_id.clone(),
                QueueKind::Steer,
                steering_mode,
            )),
            get_follow_up_messages: Some(make_queue_getter(
                shared.clone(),
                run_id.clone(),
                QueueKind::FollowUp,
                follow_up_mode,
            )),
            tool_execution: snapshot.tool_execution,
            before_tool_call: Some(before_tool_hook(
                &snapshot.hooks,
                &hook_gate,
                &hook_context,
                &run_id,
            )),
            after_tool_call: Some(after_tool_hook(
                &snapshot.hooks,
                &hook_gate,
                &hook_context,
                &run_id,
            )),
            prepare_request: None,
        };
        let agent_state = AgentState {
            system_prompt: snapshot.system_prompt.clone().unwrap_or_default(),
            model: snapshot.model.clone(),
            thinking_level: snapshot.thinking_level,
            tools,
            messages: history,
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: Default::default(),
            error_message: None,
        };
        let signal = tokio_util::sync::CancellationToken::new();
        let agent = Arc::new(Agent::new(
            agent_state,
            loop_config,
            snapshot.stream_fn.clone(),
        ));
        let listener_id = agent.subscribe(make_mirroring_listener(
            shared.clone(),
            run_id.clone(),
            snapshot.hooks.clone(),
            hook_gate.clone(),
            hook_context.clone(),
        ));
        {
            let mut engine = shared
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *engine = Some(EngineHandle {
                run_id: run_id.clone(),
                signal: signal.clone(),
                agent: agent.clone(),
                gate_control: hook_gate_control.clone(),
            });
        }
        let _ = shared.busy.send(true);
        self.events().emit(&HarnessEvent::RunStart(RunStartEvent {
            lane: "main".to_string(),
            run_id: run_id.clone(),
        }));

        RunEngine {
            agent,
            listener_id,
            signal,
        }
    }

    /// mid-run 阈值压缩闭包(对齐 pi `_compactBeforeNextAssistantResponse`):
    /// 回合间隙估算上下文,越阈值则压缩会话并用压缩后的历史替换下一回合上下文。
    async fn prepare_next_turn_hook(
        &self,
        snapshot: &PromptSnapshot,
        gate: &Gate,
        context: &HarnessContext,
        run_id: &str,
    ) -> Option<PrepareNextTurnFn> {
        let session = self.session().clone();
        let model = snapshot.model.clone();
        let settings = self.get_compaction_settings().await;
        let stream_fn = snapshot.stream_fn.clone();
        let hooks_for_compaction = snapshot.hooks.clone();
        let compaction_gate = gate.clone();
        let compaction_context = context.clone();
        let compaction_run_id = run_id.to_string();
        let thinking_level =
            crate::agent::agent_loop::reasoning_from_thinking_level(snapshot.thinking_level);
        Some(std::sync::Arc::new(move |turn: PrepareNextTurnContext| {
            let session = session.clone();
            let model = model.clone();
            let stream_fn = stream_fn.clone();
            let hooks = hooks_for_compaction.clone();
            let gate = compaction_gate.clone();
            let context = compaction_context.clone();
            let compaction_run_id = compaction_run_id.clone();
            Box::pin(async move {
                if !settings.enabled || model.context_window <= 0 {
                    return None;
                }
                let tokens = guarded_context_tokens(
                    &turn.context.messages,
                    session_latest_compaction_timestamp(&session).await,
                );
                if tokens == 0
                    || !compaction_mod::should_compact(tokens, model.context_window, &settings)
                {
                    return None;
                }
                run_auto_compaction(
                    &session,
                    settings,
                    &model,
                    thinking_level,
                    &stream_fn,
                    CompactionReason::Threshold,
                    &hooks,
                    &gate,
                    &context,
                    &compaction_run_id,
                )
                .await
                .ok()?;
                let rebuilt = build_history(&session).await.ok()?;
                Some(AgentLoopTurnUpdate {
                    context: Some(AgentContext {
                        messages: rebuilt.messages,
                        ..turn.context
                    }),
                    model: None,
                    thinking_level: None,
                })
            })
        }))
    }
    /// 阶段 7:before_drive gate → 运行 → retry_loop(溢出恢复 + 瞬态重试)
    /// → post_run_compaction → before_run_end_loop(follow-up 续跑)。
    /// 失败出口统一经 fail_run 收尾并返回 Some(result)。
    async fn drive_run(&self, prep: &PromptRunPrep, engine: &RunEngine) -> Option<RunResult> {
        let snapshot = &prep.snapshot;
        let shared = &prep.shared;
        let run_id = prep.run_id.clone();
        let hook_gate = prep.hook_gate.clone();
        let hook_context = prep.hook_context.clone();
        let agent = engine.agent.clone();
        // 7. before_drive + 运行 + 会话级重试链(对齐 AgentSession._prepareRetry)。
        if snapshot
            .hooks
            .run_with_gate(
                HookName::BeforeDrive,
                HookEvent::BeforeDrive(BeforeDriveEvent {
                    lane: "main".to_string(),
                    run_id: run_id.clone(),
                    operation: DriveOperation::Run,
                }),
                &hook_gate,
                hook_context.clone(),
            )
            .await
            .is_err()
        {
            return Some(
                self.fail_run(
                    shared,
                    &run_id,
                    OperationError {
                        code: "hook_gate".to_string(),
                        message: "Hook gate rejected drive".to_string(),
                    },
                )
                .await,
            );
        }
        if let Err(error) = agent.prompt(prep.prompt_messages.clone()).await {
            return Some(
                self.fail_run(
                    shared,
                    &run_id,
                    OperationError {
                        code: "engine_error".to_string(),
                        message: error,
                    },
                )
                .await,
            );
        }
        self.retry_loop(prep, engine).await;
        self.post_run_compaction(prep, engine).await;
        self.before_run_end_loop(prep, engine).await
    }

    /// 7a. 溢出恢复(压缩 + 重试,每次 run 只试一次,对齐 pi
    /// `_overflowRecoveryAttempted`)+ 瞬态错误重试链(retry 策略)。
    async fn retry_loop(&self, prep: &PromptRunPrep, engine: &RunEngine) {
        let snapshot = &prep.snapshot;
        let hook_gate = prep.hook_gate.clone();
        let hook_context = prep.hook_context.clone();
        let run_id = prep.run_id.clone();
        let signal = engine.signal.clone();
        let agent = engine.agent.clone();
        let mut retry_attempt: u32 = 0;
        // 溢出恢复(压缩 + 重试)每次 run 只尝试一次(对齐 pi
        // `_overflowRecoveryAttempted`,防无限循环)。
        let mut overflow_recovery_attempted = false;
        loop {
            if signal.is_cancelled() {
                break;
            }
            let last = agent.messages().last().cloned();
            let Some(AgentMessage::Message(TypedMessage::Assistant(assistant))) = last else {
                break;
            };

            // 溢出/可恢复截断优先于普通瞬态重试(对齐 pi `_checkCompaction`
            // case 1:移除失败消息 → 压缩 → continue 一次)。
            if matches!(
                assistant.stop_reason,
                StopReason::Error | StopReason::Length
            ) && !overflow_recovery_attempted
            {
                let same_model = assistant.provider == snapshot.model.provider
                    && assistant.model == snapshot.model.id;
                let overflow =
                    same_model && is_context_overflow(&assistant, snapshot.model.context_window);
                let recoverable =
                    same_model && is_recoverable_length(&assistant, snapshot.model.max_tokens);
                if overflow || recoverable {
                    overflow_recovery_attempted = true;
                    let mut messages = agent.messages();
                    if matches!(
                        messages.last(),
                        Some(AgentMessage::Message(TypedMessage::Assistant(_)))
                    ) {
                        messages.pop();
                    }
                    agent.set_messages(messages);
                    self.auto_compact(
                        CompactionReason::Overflow,
                        &hook_gate,
                        &hook_context,
                        &run_id,
                    )
                    .await;
                    if signal.is_cancelled() {
                        break;
                    }
                    if let Some(options) = self
                        .prepare_retry_stream_options(
                            &snapshot,
                            &hook_gate,
                            &hook_context,
                            &run_id,
                            retry_attempt,
                        )
                        .await
                    {
                        agent.set_stream_options(stream_options_to_simple(&options));
                    }
                    if agent.continue_run().await.is_ok() {
                        continue;
                    }
                    break;
                }
            }

            if assistant.stop_reason != StopReason::Error {
                break;
            }
            let retry = snapshot.retry_policy;
            if !retry.enabled
                || retry_attempt >= retry.max_retries
                || !is_retryable_assistant_error(&assistant)
            {
                break;
            }
            retry_attempt += 1;
            // 失败 assistant 留在 session 历史,从引擎移除后续跑。
            let mut messages = agent.messages();
            if matches!(
                messages.last(),
                Some(AgentMessage::Message(TypedMessage::Assistant(_)))
            ) {
                messages.pop();
            }
            agent.set_messages(messages);
            if let Some(options) = self
                .prepare_retry_stream_options(
                    &snapshot,
                    &hook_gate,
                    &hook_context,
                    &run_id,
                    retry_attempt,
                )
                .await
            {
                agent.set_stream_options(stream_options_to_simple(&options));
            }
            if !sleep_with_cancel(
                retry_delay_ms(retry.base_delay_ms, retry_attempt, None),
                &signal,
            )
            .await
            {
                break;
            }
            if agent.continue_run().await.is_err() {
                break;
            }
        }
    }

    /// 7.5 run 末自动压缩检查(对齐 pi `_checkCompaction` case 2/3:只压缩不重试)。
    async fn post_run_compaction(&self, prep: &PromptRunPrep, engine: &RunEngine) {
        let snapshot = &prep.snapshot;
        let hook_gate = prep.hook_gate.clone();
        let hook_context = prep.hook_context.clone();
        let run_id = prep.run_id.clone();
        let signal = engine.signal.clone();
        let agent = engine.agent.clone();
        if !signal.is_cancelled() {
            let last = agent.messages().last().cloned();
            if let Some(AgentMessage::Message(TypedMessage::Assistant(assistant))) = last {
                if assistant.stop_reason != StopReason::Aborted {
                    let settings = self.get_compaction_settings().await;
                    let context_window = snapshot.model.context_window;
                    let same_model = assistant.provider == snapshot.model.provider
                        && assistant.model == snapshot.model.id;
                    // 静默溢出(z.ai:stop 但 usage 超窗)按 overflow 压缩
                    let silent_overflow = same_model
                        && assistant.stop_reason == StopReason::Stop
                        && is_context_overflow(&assistant, context_window);
                    if settings.enabled && context_window > 0 {
                        if silent_overflow {
                            self.auto_compact(
                                CompactionReason::Overflow,
                                &hook_gate,
                                &hook_context,
                                &run_id,
                            )
                            .await;
                        } else {
                            let direct = compaction_mod::calculate_context_tokens(&assistant.usage);
                            let tokens =
                                if assistant.stop_reason == StopReason::Error || direct == 0 {
                                    // 错误/零用量消息:按估算口径(带防重触发守卫)
                                    guarded_context_tokens(
                                        &agent.messages(),
                                        session_latest_compaction_timestamp(self.session()).await,
                                    )
                                } else {
                                    direct
                                };
                            if tokens > 0
                                && compaction_mod::should_compact(tokens, context_window, &settings)
                            {
                                self.auto_compact(
                                    CompactionReason::Threshold,
                                    &hook_gate,
                                    &hook_context,
                                    &run_id,
                                )
                                .await;
                            }
                        }
                    }
                }
            }
        }
    }

    /// 7.8 before_run_end hook;follow-up 会在同一 operation 内续跑。
    /// gate 拒绝时经 fail_run 收尾并返回 Some(result)。
    async fn before_run_end_loop(
        &self,
        prep: &PromptRunPrep,
        engine: &RunEngine,
    ) -> Option<RunResult> {
        let snapshot = &prep.snapshot;
        let shared = &prep.shared;
        let hook_gate = prep.hook_gate.clone();
        let hook_context = prep.hook_context.clone();
        let run_id = prep.run_id.clone();
        let signal = engine.signal.clone();
        let agent = engine.agent.clone();
        loop {
            if signal.is_cancelled() {
                break;
            }
            let hook = snapshot
                .hooks
                .run_with_gate(
                    HookName::BeforeRunEnd,
                    HookEvent::BeforeRunEnd(BeforeRunEndEvent {
                        lane: "main".to_string(),
                        run_id: run_id.clone(),
                        messages: agent.messages(),
                    }),
                    &hook_gate,
                    hook_context.clone(),
                )
                .await;
            match hook {
                Ok(Some(HookResult::BeforeRunEnd(Some(result)))) => {
                    let Some(follow_up) = result.follow_up else {
                        break;
                    };
                    if agent
                        .prompt(vec![AgentMessage::user_text(follow_up, now_ms())])
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(_) => break,
                Err(_) => {
                    return Some(
                        self.fail_run(
                            shared,
                            &run_id,
                            OperationError {
                                code: "hook_gate".to_string(),
                                message: "Hook gate rejected run end".to_string(),
                            },
                        )
                        .await,
                    );
                }
            }
        }
        None
    }

    /// 阶段 8:结果归约(aborted / provider error / completed)+ 收尾
    /// (退订 → finish_run → RunEnd)。
    async fn finalize_run(&self, prep: &PromptRunPrep, engine: RunEngine) -> RunResult {
        let snapshot = &prep.snapshot;
        let shared = prep.shared.clone();
        let run_id = prep.run_id.clone();
        let signal = engine.signal.clone();
        let agent = engine.agent.clone();
        let listener_id = engine.listener_id;
        let cancelled = signal.is_cancelled();
        let final_assistant = agent
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                AgentMessage::Message(TypedMessage::Assistant(assistant)) => {
                    Some(assistant.clone())
                }
                _ => None,
            });
        agent.unsubscribe(listener_id);

        let leaf_id = self.leaf_id().await;
        let (record_outcome, run_outcome, end_outcome) = if cancelled {
            let assistant = final_assistant
                .unwrap_or_else(|| synthetic_assistant(&snapshot.model, StopReason::Aborted, None));
            (
                OperationOutcome::Aborted,
                RunOutcome::Aborted {
                    leaf_id: leaf_id.clone(),
                    final_entry_id: leaf_id.clone(),
                    final_message: assistant,
                },
                RunEndOutcome::Aborted,
            )
        } else if matches!(&final_assistant, Some(assistant) if assistant.stop_reason == StopReason::Error)
        {
            let assistant = final_assistant
                .unwrap_or_else(|| synthetic_assistant(&snapshot.model, StopReason::Error, None));
            let error = OperationError {
                code: "provider_error".to_string(),
                message: assistant
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Unknown provider error".to_string()),
            };
            (
                OperationOutcome::Failed,
                RunOutcome::Failed {
                    leaf_id: leaf_id.clone(),
                    error,
                    final_entry_id: Some(leaf_id.clone()),
                    final_message: Some(assistant),
                },
                RunEndOutcome::Failed,
            )
        } else {
            let assistant = final_assistant
                .unwrap_or_else(|| synthetic_assistant(&snapshot.model, StopReason::Stop, None));
            (
                OperationOutcome::Completed,
                RunOutcome::Completed {
                    leaf_id: leaf_id.clone(),
                    final_entry_id: leaf_id.clone(),
                    final_message: assistant,
                },
                RunEndOutcome::Completed,
            )
        };
        self.finish_run(&shared, &run_id, record_outcome, None)
            .await;
        self.events().emit(&HarnessEvent::RunEnd(RunEndEvent {
            lane: "main".to_string(),
            run_id: run_id.clone(),
            outcome: end_outcome,
            leaf_id,
        }));
        Ok(run_outcome)
    }

    /// 运行失败统一出口(对齐原三处失败路径的顺序):落 operation_finished →
    /// 清引擎槽位/解除 busy → emit RunEnd(Failed)。
    async fn fail_run(
        &self,
        shared: &RuntimeShared,
        run_id: &str,
        error: OperationError,
    ) -> RunResult {
        self.finish_run(
            shared,
            run_id,
            OperationOutcome::Failed,
            Some(error.clone()),
        )
        .await;
        let leaf_id = self.leaf_id().await;
        self.events().emit(&HarnessEvent::RunEnd(RunEndEvent {
            lane: "main".to_string(),
            run_id: run_id.to_string(),
            outcome: RunEndOutcome::Failed,
            leaf_id: leaf_id.clone(),
        }));
        Ok(RunOutcome::Failed {
            leaf_id,
            error,
            final_entry_id: None,
            final_message: None,
        })
    }
    async fn prepare_retry_stream_options(
        &self,
        snapshot: &PromptSnapshot,
        gate: &Gate,
        context: &HarnessContext,
        run_id: &str,
        attempt: u32,
    ) -> Option<AgentHarnessStreamOptions> {
        let mut value =
            serde_json::to_value(&snapshot.stream_options).unwrap_or(serde_json::Value::Null);
        if let Some(HookResult::BeforeRequest(Some(result))) = snapshot
            .hooks
            .run_with_gate(
                HookName::BeforeRequest,
                HookEvent::BeforeRequest(BeforeRequestEvent {
                    lane: "main".to_string(),
                    run_id: run_id.to_string(),
                    model: format!("{}/{}", snapshot.model.provider, snapshot.model.id),
                    step: RequestStep::Assistant,
                    attempt,
                    stream_options: value.clone(),
                }),
                gate,
                context.clone(),
            )
            .await
            .ok()
            .flatten()
        {
            if let Some(next) = result.stream_options {
                value = next;
            }
        }
        serde_json::from_value(value).ok()
    }
}

fn synthetic_assistant(
    model: &Model,
    stop_reason: StopReason,
    error: Option<String>,
) -> AssistantMessage {
    AssistantMessage {
        role: "assistant".to_string(),
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        usage: Usage::default(),
        stop_reason,
        error_message: error,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ms(),
    }
}

fn resources_json(resources: &AgentHarnessResources) -> serde_json::Value {
    serde_json::json!({
        "promptTemplates": resources.prompt_templates.as_ref().map(serde_json::to_value).transpose().unwrap_or(None),
        "skills": resources.skills.as_ref().map(serde_json::to_value).transpose().unwrap_or(None),
    })
}

/// 6.2 before_request hook(首次 assistant 请求前):允许 hook 覆盖流选项。
async fn resolve_hook_stream_options(
    snapshot: &PromptSnapshot,
    gate: &Gate,
    context: &HarnessContext,
    run_id: &str,
) -> AgentHarnessStreamOptions {
    let mut value =
        serde_json::to_value(&snapshot.stream_options).unwrap_or_else(|_| serde_json::Value::Null);
    if let Some(HookResult::BeforeRequest(Some(result))) = snapshot
        .hooks
        .run_with_gate(
            HookName::BeforeRequest,
            HookEvent::BeforeRequest(BeforeRequestEvent {
                lane: "main".to_string(),
                run_id: run_id.to_string(),
                model: format!("{}/{}", snapshot.model.provider, snapshot.model.id),
                step: RequestStep::Assistant,
                attempt: 0,
                stream_options: value.clone(),
            }),
            gate,
            context.clone(),
        )
        .await
        .ok()
        .flatten()
    {
        if let Some(next) = result.stream_options {
            value = next;
        }
    }
    serde_json::from_value::<AgentHarnessStreamOptions>(value).unwrap_or_default()
}

/// BeforePayload hook → SimpleStreamOptions.on_payload 闭包。
fn payload_hook(
    snapshot: &PromptSnapshot,
    gate: &Gate,
    context: &HarnessContext,
    run_id: &str,
) -> OnPayloadFn {
    let hooks = snapshot.hooks.clone();
    let gate = gate.clone();
    let context = context.clone();
    let run_id = run_id.to_string();
    let model = format!("{}/{}", snapshot.model.provider, snapshot.model.id);
    Arc::new(move |payload: serde_json::Value| {
        let hooks = hooks.clone();
        let gate = gate.clone();
        let context = context.clone();
        let run_id = run_id.clone();
        let model = model.clone();
        Box::pin(async move {
            let Ok(Some(HookResult::BeforePayload(Some(result)))) = hooks
                .run_with_gate(
                    HookName::BeforePayload,
                    HookEvent::BeforePayload(BeforePayloadEvent {
                        lane: "main".to_string(),
                        run_id,
                        model,
                        payload,
                    }),
                    &gate,
                    context,
                )
                .await
            else {
                return None;
            };
            Some(result.payload)
        })
    })
}

/// BeforeTool hook → AgentLoopConfig.before_tool_call 闭包。
fn before_tool_hook(
    hooks: &Arc<HookRegistry>,
    gate: &Gate,
    context: &HarnessContext,
    run_id: &str,
) -> BeforeToolCallHookFn {
    let hooks = hooks.clone();
    let gate = gate.clone();
    let context = context.clone();
    let run_id = run_id.to_string();
    Arc::new(move |call, signal| {
        let hooks = hooks.clone();
        let gate = gate.clone();
        let context = context.clone();
        let run_id = run_id.clone();
        Box::pin(async move {
            let context = context.with_signal_opt(signal);
            let hook_result = hooks
                .run_tool_with_gate(
                    HookName::BeforeTool,
                    HookEvent::BeforeTool(BeforeToolEvent {
                        lane: "main".to_string(),
                        run_id,
                        tool_call_id: call.tool_call.id.clone(),
                        tool_name: call.tool_call.name.clone(),
                        args: call.args.clone(),
                    }),
                    &gate,
                    context,
                )
                .await;
            match hook_result {
                Ok(Some(HookResult::BeforeTool(result))) => {
                    let result = result.unwrap_or_default();
                    Some(BeforeToolCallResult {
                        args: result.args,
                        block: result.block.is_some(),
                        reason: result.block.as_ref().map(|block| block.reason.clone()),
                        terminate: result
                            .block
                            .and_then(|block| block.terminate)
                            .unwrap_or(false),
                    })
                }
                Ok(_) => Some(BeforeToolCallResult::default()),
                Err(error) => Some(BeforeToolCallResult {
                    args: None,
                    block: true,
                    reason: Some(error.message),
                    terminate: false,
                }),
            }
        })
    })
}

/// AfterTool hook → AgentLoopConfig.after_tool_call 闭包。
fn after_tool_hook(
    hooks: &Arc<HookRegistry>,
    gate: &Gate,
    context: &HarnessContext,
    run_id: &str,
) -> AfterToolCallHookFn {
    let hooks = hooks.clone();
    let gate = gate.clone();
    let context = context.clone();
    let run_id = run_id.to_string();
    Arc::new(move |call, signal| {
        let hooks = hooks.clone();
        let gate = gate.clone();
        let context = context.clone();
        let run_id = run_id.clone();
        Box::pin(async move {
            let context = context.with_signal_opt(signal);
            let details = if call.result.details.is_null() {
                None
            } else {
                Some(call.result.details.clone())
            };
            let hook_result = hooks
                .run_tool_with_gate(
                    HookName::AfterTool,
                    HookEvent::AfterTool(AfterToolEvent {
                        lane: "main".to_string(),
                        run_id,
                        tool_call_id: call.tool_call.id.clone(),
                        tool_name: call.tool_call.name.clone(),
                        args: call.args.clone(),
                        content: call.result.content.clone(),
                        details,
                        is_error: call.is_error,
                        usage: call.result.usage.clone(),
                    }),
                    &gate,
                    context,
                )
                .await;
            match hook_result {
                Ok(Some(HookResult::AfterTool(Some(result)))) => Some(AfterToolCallResult {
                    content: result.content,
                    details: result.details,
                    is_error: result.is_error,
                    usage: result.usage,
                    terminate: result.terminate,
                }),
                _ => Some(AfterToolCallResult::default()),
            }
        })
    })
}
