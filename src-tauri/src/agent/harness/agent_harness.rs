//! AgentHarness 组合层:对齐 `packages/agent/src/harness/agent-harness.ts`。
//!
//! 蓝本在 0.84.4 为显式 scaffold;本仓库在保持其公开契约(类型/错误/结果形状)
//! 的前提下接入了运行时(runtime.rs):prompt/abort/steer/followUp/nextRun/
//! cancelQueued/recordUsage/waitForIdle/runWhenIdle/runToCompletion/watch/
//! watchSession/lane/lanes/compact 可用;resume 在 create 恢复后无挂起 operation
//! 可续(崩溃 operation 已归约 aborted),返回 `NothingToResume`。
//! 仍按蓝本 NotImplemented 的:skill/promptFromTemplate(资源未接线)、
//! navigateTree、peekAction/executeAction(manual drive)、createLane(单 lane)。

use crate::agent::agent::{default_convert_to_llm_fn, Agent};
use crate::agent::agent_loop::now_ms;
use crate::agent::harness::compaction::compaction::{self as compaction_mod, CompactionSettings};
use crate::agent::harness::errors::{
    Closed, HarnessClosed, HarnessNotImplemented, HarnessUnavailable, InvalidLane, InvalidMessage,
    LaneBusy, LaneExists, MissingIdentities, NoActiveOperation, NoActiveRun, NothingToCompact,
    NothingToResume, OperationError, UnknownQueueItem, UnknownSkill, UnknownTarget,
    UnknownTemplate,
};
use crate::agent::harness::events::{
    HarnessEvent, HarnessEventBus, HarnessEventListener, HarnessEventType, RunEndEvent,
    RunEndOutcome, RunStartEvent, WatchHandle,
};
use crate::agent::harness::runtime::{
    branch_entries, build_history, make_mirroring_listener, make_queue_getter, operation_error,
    stream_options_to_simple, EmptyToolContext, EngineHandle, QueueSet, QueuedEntry, RuntimeShared,
};
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    BranchSummaryEntry, CompactionEntry, CompactionReason, Entry, LaneRecord, OperationIntent,
    OperationOutcome, OperationStartedRecord, ProvisionedEntry, ProvisionedMessageEntry,
    QueueCancelledRecord, QueueEnqueuedRecord, QueueKind, RecordQuery, SessionError, SessionTree,
    StepAttemptRecord, StepKind, UsageCauseKind, UsageRecord,
};
use crate::agent::harness::telemetry::TelemetryContext;
use crate::agent::harness::types::{
    AgentHarnessResources, AgentHarnessStreamOptions, AgentHarnessStreamOptionsPatch,
    Result as ResultValue, ToolContext,
};
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::retry::{is_retryable_assistant_error, retry_delay_ms, sleep_with_cancel};
use crate::agent::llm::types::{AssistantMessage, Model, ModelThinkingLevel, StopReason, Usage};
use crate::agent::types::{
    AgentLoopConfig, AgentMessage, AgentState, QueueMode, ToolExecutionMode, TypedMessage,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// 结果类型(对齐 TS RunOutcome/... 与各 Rejected 联合)
// ---------------------------------------------------------------------------

/// run 完成结果(对齐 TS `RunOutcome`)。
pub enum RunOutcome {
    Completed {
        leaf_id: String,
        final_entry_id: String,
        final_message: AssistantMessage,
    },
    Aborted {
        leaf_id: String,
        final_entry_id: String,
        final_message: AssistantMessage,
    },
    Failed {
        leaf_id: String,
        error: OperationError,
        final_entry_id: Option<String>,
        final_message: Option<AssistantMessage>,
    },
    /// 蓝本 `suspended` 依赖 DeferredHandle(未建模,见报告偏差),形状简化为
    /// leaf/finalEntry。
    Suspended {
        leaf_id: String,
        final_entry_id: String,
    },
}

/// compaction 结果(对齐 TS `CompactionOutcome`)。
pub enum CompactionOutcome {
    Completed {
        leaf_id: String,
        entry: Box<CompactionEntry>,
    },
    DeclinedOrAborted {
        leaf_id: String,
    },
    Failed {
        leaf_id: String,
        error: OperationError,
    },
}

/// 导航结果(对齐 TS `NavigationOutcome`)。
pub enum NavigationOutcome {
    Completed {
        new_leaf_id: Option<String>,
        summary_entry: Option<Box<BranchSummaryEntry>>,
    },
    DeclinedOrAborted {
        leaf_id: Option<String>,
    },
    Failed {
        leaf_id: Option<String>,
        error: OperationError,
    },
}

/// run 拒绝原因(对齐 TS `RunRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum RunRejected {
    #[error(transparent)]
    LaneBusy(#[from] LaneBusy),
    #[error(transparent)]
    InvalidMessage(#[from] InvalidMessage),
    #[error(transparent)]
    UnknownSkill(#[from] UnknownSkill),
    #[error(transparent)]
    UnknownTemplate(#[from] UnknownTemplate),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// compaction 拒绝原因(对齐 TS `CompactionRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum CompactionRejected {
    #[error(transparent)]
    LaneBusy(#[from] LaneBusy),
    #[error(transparent)]
    NothingToCompact(#[from] NothingToCompact),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// 导航拒绝原因(对齐 TS `NavigationRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum NavigationRejected {
    #[error(transparent)]
    LaneBusy(#[from] LaneBusy),
    #[error(transparent)]
    UnknownTarget(#[from] UnknownTarget),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// resume 拒绝原因(对齐 TS `ResumeRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum ResumeRejected {
    #[error(transparent)]
    LaneBusy(#[from] LaneBusy),
    #[error(transparent)]
    NothingToResume(#[from] NothingToResume),
    #[error(transparent)]
    MissingIdentities(#[from] MissingIdentities),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// 队列拒绝原因(对齐 TS `QueueRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum QueueRejected {
    #[error(transparent)]
    NoActiveRun(#[from] NoActiveRun),
    #[error(transparent)]
    InvalidMessage(#[from] InvalidMessage),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// 取消排队拒绝原因(对齐 TS `CancelQueuedRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum CancelQueuedRejected {
    #[error(transparent)]
    UnknownQueueItem(#[from] UnknownQueueItem),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// abort 拒绝原因(对齐 TS `AbortRejected`)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum AbortRejected {
    #[error(transparent)]
    NoActiveOperation(#[from] NoActiveOperation),
    #[error(transparent)]
    Closed(#[from] Closed),
}

/// 建 lane 拒绝原因(对齐 TS `CreateLaneResult` 的错误联合)。
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum CreateLaneRejected {
    #[error(transparent)]
    LaneExists(#[from] LaneExists),
    #[error(transparent)]
    InvalidLane(#[from] InvalidLane),
    #[error(transparent)]
    UnknownTarget(#[from] UnknownTarget),
    #[error(transparent)]
    Closed(#[from] Closed),
}

pub type RunResult = ResultValue<RunOutcome, RunRejected>;
pub type CompactionResult = ResultValue<CompactionOutcome, CompactionRejected>;
pub type NavigationResult = ResultValue<NavigationOutcome, NavigationRejected>;
pub type QueueResult = ResultValue<String, QueueRejected>;
pub type CancelQueuedResult = ResultValue<CancelQueuedOutcome, CancelQueuedRejected>;
pub type RecordUsageResult = ResultValue<(), Closed>;
pub type AbortResult = ResultValue<AbortOutcome, AbortRejected>;
pub type ResumeResult = ResultValue<ResumeOutcome, ResumeRejected>;
pub type CreateLaneResult = ResultValue<Lane, CreateLaneRejected>;

/// 取消排队结果(对齐 TS 字面量联合)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelQueuedOutcome {
    Cancelled,
    AlreadyConsumed,
    AlreadyCleared,
}

/// abort 结果(对齐 TS `{runId, steer, followUp}`)。
pub struct AbortOutcome {
    pub run_id: String,
    pub steer: Vec<AgentMessage>,
    pub follow_up: Vec<AgentMessage>,
}

/// resume 结果(对齐 TS `ResumeOutcome`;WIP 骨架仅保留类型形状)。
pub enum ResumeOutcome {
    Run {
        run_id: String,
        outcome: Box<RunOutcome>,
    },
    Compaction {
        run_id: String,
        outcome: Box<CompactionOutcome>,
    },
    Navigation {
        run_id: String,
        outcome: Box<NavigationOutcome>,
    },
}

// ---------------------------------------------------------------------------
// 选项 / 快照形状
// ---------------------------------------------------------------------------

/// 导航选项(对齐 TS `NavigateOptions`)。
#[derive(Clone, Debug, Default)]
pub struct NavigateOptions {
    pub summarize: Option<bool>,
    pub custom_instructions: Option<String>,
    pub label: Option<String>,
}

/// operation 种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKind {
    Run,
    Compaction,
    Navigation,
}

/// operation 状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationStatus {
    Running,
    Suspended,
    Aborting,
}

/// lane 概要(对齐 TS `LaneInfo`)。
#[derive(Clone, Debug)]
pub struct Lane {
    pub name: String,
    pub leaf_id: Option<String>,
    pub operation: Option<LaneOperationInfo>,
}

/// lane 上的 operation 概要。
#[derive(Clone, Debug)]
pub struct LaneOperationInfo {
    pub id: String,
    pub kind: OperationKind,
    pub status: OperationStatus,
}

/// 排队条目(对齐 TS `QueuedItem`)。
pub struct QueuedItem {
    pub entry_id: String,
    pub message: AgentMessage,
}

/// 队列快照。
pub struct QueuesSnapshot {
    pub steer: Vec<QueuedItem>,
    pub follow_up: Vec<QueuedItem>,
    pub next_run: Vec<QueuedItem>,
}

/// lane 快照(对齐 TS `LaneSnapshot`)。
pub struct LaneSnapshot {
    pub lane: String,
    pub transcript: Vec<Entry>,
    pub leaf_id: Option<String>,
    pub operation: Option<LaneOperationInfo>,
    pub queues: QueuesSnapshot,
    pub pending_writes: Vec<(String, ProvisionedEntry)>,
    pub faulted: bool,
}

/// 挂起 operation 概要(蓝本 SuspendedOperation;deferred 句柄未建模)。
pub struct SuspendedOperation {
    pub lane: String,
    pub kind: OperationKind,
    pub id: String,
    pub started_at: i64,
    pub reason: SuspensionReason,
    pub prompt: Option<Vec<AgentMessage>>,
    pub aborting: Option<AbortOutcome>,
    /// (缺失的工具名, 缺失的模型名)。
    pub missing: (Vec<String>, Vec<String>),
}

/// 挂起原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuspensionReason {
    Crash,
    Deferred,
}

/// 会话快照(对齐 TS `SessionSnapshot`)。
pub struct SessionSnapshot {
    pub lanes: Vec<(Lane, Option<SuspendedOperation>)>,
    pub faulted: bool,
}

/// harness 钩子名(对齐 TS `HookName`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookName {
    BeforeRun,
    BeforeResume,
    BeforeRunEnd,
    TransformContext,
    BeforeRequest,
    BeforePayload,
    AfterResponse,
    BeforeTool,
    AfterTool,
    BeforeCompaction,
    BeforeNavigation,
}

impl HookName {
    pub fn as_str(&self) -> &'static str {
        match self {
            HookName::BeforeRun => "before_run",
            HookName::BeforeResume => "before_resume",
            HookName::BeforeRunEnd => "before_run_end",
            HookName::TransformContext => "transform_context",
            HookName::BeforeRequest => "before_request",
            HookName::BeforePayload => "before_payload",
            HookName::AfterResponse => "after_response",
            HookName::BeforeTool => "before_tool",
            HookName::AfterTool => "after_tool",
            HookName::BeforeCompaction => "before_compaction",
            HookName::BeforeNavigation => "before_navigation",
        }
    }
}

/// 重试策略(蓝本由 pi-ai 提供;本复刻在 harness 侧定义,与 coding-agent 的
/// `settings.retry` 同形:`baseDelayMs * 2^(attempt-1)`)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetryPolicy {
    pub enabled: bool,
    /// 最大重试次数(0 = 不重试;首次调用不计入)。
    pub max_retries: u32,
    /// 基础退避毫秒。
    pub base_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_retries: 0,
            base_delay_ms: 1000,
        }
    }
}

/// harness 构造选项(对齐 TS `AgentHarnessOptions`)。
pub struct AgentHarnessOptions {
    pub session: Session,
    pub stream_fn: crate::agent::types::StreamFn,
    pub model: Model,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub active_tool_names: Option<Vec<String>>,
    pub tools: Vec<crate::agent::harness::types::AgentHarnessTool>,
    /// 静态工具上下文(每回合解析器形式未接入,与蓝本 WIP 一致)。
    pub tool_context: Option<std::sync::Arc<dyn crate::agent::harness::types::ToolContext>>,
    pub system_prompt: Option<String>,
    pub resources: AgentHarnessResources,
    pub stream_options: AgentHarnessStreamOptions,
    pub retry: Option<RetryPolicy>,
    pub compaction: Option<CompactionSettings>,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub tool_execution: ToolExecutionMode,
    pub telemetry_context: Option<std::sync::Arc<dyn TelemetryContext>>,
}

// ---------------------------------------------------------------------------
// AgentHarness
// ---------------------------------------------------------------------------

/// AgentHarness:与蓝本相同的公开契约;运行方法经 runtime.rs 接线
/// (蓝本 scaffold 未提供实现,运行语义见 runtime.rs 模块注释)。
pub struct AgentHarness {
    name: &'static str,
    session: Session,
    events: HarnessEventBus,
    state: Mutex<HarnessState>,
}

struct HarnessState {
    model: Model,
    thinking_level: ModelThinkingLevel,
    active_tool_names: Vec<String>,
    tools: Vec<crate::agent::harness::types::AgentHarnessTool>,
    resources: AgentHarnessResources,
    stream_options: AgentHarnessStreamOptions,
    retry_policy: RetryPolicy,
    compaction_settings: CompactionSettings,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
    closed: bool,
    // ---- 运行期接线(蓝本构造时仅存档,本仓库实际使用) ----
    stream_fn: Option<crate::agent::types::StreamFn>,
    system_prompt: Option<String>,
    tool_context: Option<Arc<dyn ToolContext>>,
    tool_execution: ToolExecutionMode,
    telemetry_context: Option<Arc<dyn TelemetryContext>>,
    queues: Arc<Mutex<QueueSet>>,
    engine: Arc<Mutex<Option<EngineHandle>>>,
    busy: Arc<tokio::sync::watch::Sender<bool>>,
}

impl AgentHarness {
    /// 创建 harness(对齐 TS `create`)。存在历史记录时经 reducer 重建状态:
    /// 未完结的 operation 合成 aborted 收尾并以 [`SuspendedOperation`] 返回
    /// (蓝本抛 `create.restore`;本仓库为实现恢复语义的扩展,原因见 runtime.rs)。
    pub async fn create(
        options: AgentHarnessOptions,
    ) -> Result<(Self, Vec<SuspendedOperation>), HarnessNotImplemented> {
        let suspended = Self::restore_open_operations(&options.session)
            .await
            .map_err(|error| HarnessNotImplemented::new(format!("create.restore({error})")))?;
        let AgentHarnessOptions {
            session,
            stream_fn,
            model,
            thinking_level,
            active_tool_names,
            tools,
            tool_context,
            system_prompt,
            resources,
            stream_options,
            retry,
            compaction,
            steering_mode,
            follow_up_mode,
            tool_execution,
            telemetry_context,
        } = options;
        let (queues, engine, busy) = {
            let shared = RuntimeShared::new(session.clone());
            (
                shared.queues.clone(),
                shared.engine.clone(),
                shared.busy.clone(),
            )
        };
        let active_tool_names = active_tool_names
            .unwrap_or_else(|| tools.iter().map(|tool| tool.name.clone()).collect());
        Ok((
            Self {
                name: "main",
                session,
                events: HarnessEventBus::new(),
                state: Mutex::new(HarnessState {
                    model,
                    thinking_level: thinking_level.unwrap_or(ModelThinkingLevel::Off),
                    active_tool_names,
                    tools,
                    resources: AgentHarnessResources {
                        prompt_templates: resources.prompt_templates,
                        skills: resources.skills,
                    },
                    stream_options,
                    retry_policy: retry.unwrap_or_default(),
                    compaction_settings: compaction.unwrap_or(
                        crate::agent::harness::compaction::compaction::DEFAULT_COMPACTION_SETTINGS,
                    ),
                    steering_mode,
                    follow_up_mode,
                    closed: false,
                    stream_fn: Some(stream_fn),
                    system_prompt,
                    tool_context,
                    tool_execution,
                    telemetry_context,
                    queues,
                    engine,
                    busy,
                }),
            },
            suspended,
        ))
    }

    /// 恢复辅助:把未完结 operation 归约为 aborted 并返回挂起概要。
    async fn restore_open_operations(
        session: &Session,
    ) -> Result<Vec<SuspendedOperation>, SessionError> {
        let open = session.find_open_operations("main", None).await?;
        let mut suspended = Vec::new();
        for record in open {
            let kind = match &record.intent {
                OperationIntent::Run { .. } => OperationKind::Run,
                OperationIntent::Compaction { .. } => OperationKind::Compaction,
                OperationIntent::Navigation { .. } => OperationKind::Navigation,
            };
            let prompt = match &record.intent {
                OperationIntent::Run {
                    original_prompt, ..
                } => Some(original_prompt.clone()),
                _ => None,
            };
            session
                .append_record(LaneRecord::OperationFinished(
                    crate::agent::harness::session::types::OperationFinishedRecord {
                        id: uuid_v7(),
                        seq: 0,
                        lane: "main".to_string(),
                        timestamp: now_ms(),
                        run_id: record.id.clone(),
                        outcome: OperationOutcome::Aborted,
                        error: Some(OperationError {
                            code: "crash_restored".to_string(),
                            message: "Operation was interrupted before completion and was restored as aborted."
                                .to_string(),
                        }),
                    },
                ))
                .await?;
            suspended.push(SuspendedOperation {
                lane: "main".to_string(),
                kind,
                id: record.id.clone(),
                started_at: record.timestamp,
                reason: SuspensionReason::Crash,
                prompt,
                aborting: None,
                missing: (Vec::new(), Vec::new()),
            });
        }
        Ok(suspended)
    }

    fn unavailable<T>(&self, operation: &str) -> Result<T, HarnessUnavailable> {
        if self.is_closed() {
            Err(HarnessClosed.into())
        } else {
            Err(HarnessNotImplemented::new(operation).into())
        }
    }

    fn is_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .closed
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// 事件总线(事件经 emit 分发到当前 run 的监听方;类型见 events.rs 的
    /// `HarnessEventType`,hooks 仍按蓝本未接线)。
    pub fn events(&self) -> &HarnessEventBus {
        &self.events
    }

    /// 按事件类型注册监听(对齐 TS `events.on(type, listener)`),返回退订
    /// 闭包;只投递注册之后发出的事件,不回放历史。
    pub fn on_event(
        &self,
        event_type: HarnessEventType,
        listener: HarnessEventListener,
    ) -> Box<dyn FnOnce() + Send> {
        self.events.on(event_type, listener)
    }

    pub fn emit_event(&self, event: &HarnessEvent) {
        self.events.emit(event);
    }

    pub async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.session.get_leaf_id().await
    }

    // ----- 运行方法(runtime.rs 接线) -----

    /// 当前运行期共享依赖(从既有字段拼装)。
    fn shared(&self) -> RuntimeShared {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RuntimeShared {
            session: self.session.clone(),
            bus: self.events.clone(),
            queues: state.queues.clone(),
            engine: state.engine.clone(),
            busy: state.busy.clone(),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, HarnessState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn closed_error() -> RunRejected {
        RunRejected::Closed(Closed::new("AgentHarness was closed"))
    }

    async fn leaf_id(&self) -> String {
        self.session
            .get_leaf_id()
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// main lane 的在途 operation 概要(当前仅 run 一种)。
    async fn main_lane_operation(&self) -> Option<LaneOperationInfo> {
        let state = self.lock_state();
        let engine = state
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        engine.as_ref().map(|engine| LaneOperationInfo {
            id: engine.run_id.clone(),
            kind: OperationKind::Run,
            status: OperationStatus::Running,
        })
    }

    async fn main_lane(&self) -> Lane {
        Lane {
            name: "main".to_string(),
            leaf_id: Some(self.leaf_id().await),
            operation: self.main_lane_operation().await,
        }
    }

    /// 落 operation_finished、清引擎槽位、解除 busy。
    async fn finish_run(
        &self,
        shared: &RuntimeShared,
        run_id: &str,
        outcome: OperationOutcome,
        error: Option<OperationError>,
    ) {
        let _ = self
            .session
            .append_record(LaneRecord::OperationFinished(
                crate::agent::harness::session::types::OperationFinishedRecord {
                    id: uuid_v7(),
                    seq: 0,
                    lane: "main".to_string(),
                    timestamp: now_ms(),
                    run_id: run_id.to_string(),
                    outcome,
                    error,
                },
            ))
            .await;
        *shared
            .engine
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let _ = shared.busy.send(false);
    }

    pub async fn prompt(&self, text: String) -> RunResult {
        self.prompt_input(QueueInput::Text(text)).await
    }

    async fn prompt_input(&self, input: QueueInput) -> RunResult {
        // 1. 守卫 + 配置快照(短临界区,不跨 await)。
        let snapshot = {
            let state = self.lock_state();
            if state.closed {
                return Err(Self::closed_error());
            }
            let engine = state
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(engine) = engine.as_ref() {
                return Err(RunRejected::LaneBusy(LaneBusy::new(
                    format!("Lane main is busy with operation {}", engine.run_id),
                    "main".to_string(),
                    engine.run_id.clone(),
                    "run".to_string(),
                )));
            }
            PromptSnapshot {
                model: state.model.clone(),
                thinking_level: state.thinking_level,
                active_tool_names: state.active_tool_names.clone(),
                tools: state.tools.clone(),
                tool_context: state.tool_context.clone(),
                system_prompt: state.system_prompt.clone(),
                tool_execution: state.tool_execution,
                stream_options: state.stream_options.clone(),
                retry_policy: state.retry_policy,
                stream_fn: state
                    .stream_fn
                    .clone()
                    .expect("stream_fn is set at create time"),
            }
        };

        // 2. 输入归一化。
        let prompt_messages: Vec<AgentMessage> = match input {
            QueueInput::Text(text) => vec![AgentMessage::user_text(text, now_ms())],
            QueueInput::Message(message) => vec![*message],
            QueueInput::Messages(messages) => messages,
        };
        if prompt_messages.is_empty() {
            return Err(RunRejected::InvalidMessage(InvalidMessage::new(
                "Prompt must contain at least one message",
                "main".to_string(),
                "empty prompt".to_string(),
            )));
        }

        let shared = self.shared();
        let run_id = uuid_v7();

        // 3. nextRun 捕获项 + durable intent 落库。
        let initial: Vec<QueuedEntry> = {
            let mut queues = shared
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            QueueSet::drain(&mut queues.next_run, QueueMode::All)
        };
        let accept = self
            .session
            .append_record(LaneRecord::OperationStarted(OperationStartedRecord {
                id: run_id.clone(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                source_leaf_id: self.session.get_leaf_id().await.unwrap_or(None),
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
            return Ok(RunOutcome::Failed {
                leaf_id: self.leaf_id().await,
                error: operation_error(error),
                final_entry_id: None,
                final_message: None,
            });
        }
        // 4. nextRun 前置条目物化。
        for item in &initial {
            let _ = self
                .session
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

        // 5. 历史(不含本次 prompt;prompt 由引擎经事件循环落库)。
        let history = match build_history(&self.session).await {
            Ok(history) => history.messages,
            Err(error) => {
                return Ok(RunOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error: operation_error(error),
                    final_entry_id: None,
                    final_message: None,
                });
            }
        };

        // 6. 组装引擎 Agent。
        let (steering_mode, follow_up_mode) = {
            let state = self.lock_state();
            (state.steering_mode, state.follow_up_mode)
        };
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
        let loop_config = AgentLoopConfig {
            model: snapshot.model.clone(),
            stream: stream_options_to_simple(&snapshot.stream_options),
            convert_to_llm: default_convert_to_llm_fn(),
            transform_context: None,
            get_api_key: None,
            should_stop_after_turn: None,
            prepare_next_turn: None,
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
            before_tool_call: None,
            after_tool_call: None,
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
        let listener_id = agent.subscribe(make_mirroring_listener(shared.clone(), run_id.clone()));
        {
            let mut engine = shared
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *engine = Some(EngineHandle {
                run_id: run_id.clone(),
                signal: signal.clone(),
                agent: agent.clone(),
            });
        }
        let _ = shared.busy.send(true);
        self.events.emit(&HarnessEvent::RunStart(RunStartEvent {
            lane: "main".to_string(),
            run_id: run_id.clone(),
        }));

        // 7. 运行 + 会话级重试链(对齐 AgentSession._prepareRetry)。
        if let Err(error) = agent.prompt(prompt_messages).await {
            let message = error.clone();
            self.finish_run(
                &shared,
                &run_id,
                OperationOutcome::Failed,
                Some(OperationError {
                    code: "engine_error".to_string(),
                    message: message.clone(),
                }),
            )
            .await;
            let leaf_id = self.leaf_id().await;
            self.events.emit(&HarnessEvent::RunEnd(RunEndEvent {
                lane: "main".to_string(),
                run_id: run_id.clone(),
                outcome: RunEndOutcome::Failed,
                leaf_id: leaf_id.clone(),
            }));
            return Ok(RunOutcome::Failed {
                leaf_id,
                error: OperationError {
                    code: "engine_error".to_string(),
                    message,
                },
                final_entry_id: None,
                final_message: None,
            });
        }
        let mut retry_attempt: u32 = 0;
        loop {
            if signal.is_cancelled() {
                break;
            }
            let last = agent.messages().last().cloned();
            let Some(AgentMessage::Message(TypedMessage::Assistant(assistant))) = last else {
                break;
            };
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
            if !sleep_with_cancel(retry_delay_ms(retry.base_delay_ms, retry_attempt), &signal).await
            {
                break;
            }
            if agent.continue_run().await.is_err() {
                break;
            }
        }

        // 8. 结果归约 + 收尾。
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
        self.events.emit(&HarnessEvent::RunEnd(RunEndEvent {
            lane: "main".to_string(),
            run_id: run_id.clone(),
            outcome: end_outcome,
            leaf_id,
        }));
        Ok(run_outcome)
    }

    pub async fn skill(
        &self,
        _name: String,
        _additional_instructions: Option<String>,
    ) -> Result<RunOutcome, HarnessUnavailable> {
        self.unavailable("skill")
    }

    pub async fn prompt_from_template(
        &self,
        _name: String,
        _args: Option<Vec<String>>,
    ) -> Result<RunOutcome, HarnessUnavailable> {
        self.unavailable("promptFromTemplate")
    }

    /// 手动 compaction:接 compaction 模块(prepare → 摘要 → compaction 条目)。
    pub async fn compact(
        &self,
        options: Option<CompactOptions>,
    ) -> Result<CompactionOutcome, HarnessUnavailable> {
        let shared = self.shared();
        let (settings, model, thinking_level, stream_fn) = {
            let state = self.lock_state();
            if state.closed {
                return Err(HarnessClosed.into());
            }
            if state
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some()
            {
                return Err(HarnessUnavailable::from(HarnessNotImplemented::new(
                    "compact.busy",
                )));
            }
            (
                state.compaction_settings,
                state.model.clone(),
                crate::agent::agent_loop::reasoning_from_thinking_level(state.thinking_level),
                state
                    .stream_fn
                    .clone()
                    .expect("stream_fn is set at create time"),
            )
        };
        let run_id = uuid_v7();
        let result_entry_id = uuid_v7();
        let custom_instructions = options.and_then(|options| options.custom_instructions);

        // intent 落库。
        if let Err(error) = self
            .session
            .append_record(LaneRecord::OperationStarted(OperationStartedRecord {
                id: run_id.clone(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                source_leaf_id: self.session.get_leaf_id().await.unwrap_or(None),
                intent: OperationIntent::Compaction {
                    custom_instructions: custom_instructions.clone(),
                    result_entry_id: result_entry_id.clone(),
                },
            }))
            .await
        {
            return Ok(CompactionOutcome::Failed {
                leaf_id: self.leaf_id().await,
                error: operation_error(error),
            });
        }
        let _ = shared.busy.send(true);

        // 准备 + 摘要。
        let entries = match branch_entries(&self.session).await {
            Ok(entries) => entries,
            Err(error) => {
                self.finish_run(
                    &shared,
                    &run_id,
                    OperationOutcome::Failed,
                    Some(operation_error(error)),
                )
                .await;
                return Ok(CompactionOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error: OperationError {
                        code: "session_error".to_string(),
                        message: "failed to read session branch".to_string(),
                    },
                });
            }
        };
        let preparation = match compaction_mod::prepare_compaction(&entries, settings) {
            Ok(Some(preparation)) => preparation,
            Ok(None) => {
                self.finish_run(&shared, &run_id, OperationOutcome::Declined, None)
                    .await;
                return Ok(CompactionOutcome::DeclinedOrAborted {
                    leaf_id: self.leaf_id().await,
                });
            }
            Err(error) => {
                self.finish_run(
                    &shared,
                    &run_id,
                    OperationOutcome::Failed,
                    Some(OperationError {
                        code: format!("compaction_{}", error.code),
                        message: error.message.clone(),
                    }),
                )
                .await;
                return Ok(CompactionOutcome::Failed {
                    leaf_id: self.leaf_id().await,
                    error: OperationError {
                        code: "compaction_prepare".to_string(),
                        message: error.to_string(),
                    },
                });
            }
        };
        match compaction_mod::compact(
            preparation,
            &stream_fn,
            &model,
            custom_instructions.as_deref(),
            thinking_level,
        )
        .await
        {
            Ok(result) => {
                let entry = self
                    .session
                    .append_entry(
                        ProvisionedEntry::Compaction(
                            crate::agent::harness::session::types::ProvisionedCompactionEntry {
                                id: result_entry_id.clone(),
                                summary: result.summary,
                                retained_tail: result.retained_tail,
                                tokens_before: result.tokens_before,
                                details: Some(result.details),
                                usage: Some(result.usage.clone()),
                            },
                        ),
                        "main".to_string(),
                    )
                    .await;
                let entry = match entry {
                    Ok(Entry::Compaction(entry)) => entry,
                    Ok(_) => unreachable!("compaction entry round-trips"),
                    Err(error) => {
                        self.finish_run(
                            &shared,
                            &run_id,
                            OperationOutcome::Failed,
                            Some(operation_error(error)),
                        )
                        .await;
                        return Ok(CompactionOutcome::Failed {
                            leaf_id: self.leaf_id().await,
                            error: OperationError {
                                code: "session_error".to_string(),
                                message: "failed to append compaction entry".to_string(),
                            },
                        });
                    }
                };
                let _ = self
                    .session
                    .append_record(LaneRecord::StepAttempt(StepAttemptRecord {
                        id: uuid_v7(),
                        seq: 0,
                        lane: "main".to_string(),
                        timestamp: now_ms(),
                        run_id: run_id.clone(),
                        step: StepKind::Compaction,
                        attempt: 1,
                        result_entry_id: entry.id.clone(),
                        compaction_reason: Some(CompactionReason::Manual),
                    }))
                    .await;
                let _ = self
                    .session
                    .append_record(LaneRecord::Usage(UsageRecord {
                        id: uuid_v7(),
                        seq: 0,
                        lane: "main".to_string(),
                        timestamp: now_ms(),
                        usage: result.usage,
                        cause: UsageCauseKind::Compaction,
                        run_id: Some(run_id.clone()),
                        entry_id: Some(entry.id.clone()),
                        attempt: Some(1),
                        stop_reason: None,
                        tool_call_id: None,
                        details: None,
                    }))
                    .await;
                self.finish_run(&shared, &run_id, OperationOutcome::Completed, None)
                    .await;
                Ok(CompactionOutcome::Completed {
                    leaf_id: self.leaf_id().await,
                    entry: Box::new(entry),
                })
            }
            Err(error) => {
                let aborted =
                    error.code == crate::agent::harness::types::CompactionErrorCode::Aborted;
                self.finish_run(
                    &shared,
                    &run_id,
                    if aborted {
                        OperationOutcome::Aborted
                    } else {
                        OperationOutcome::Failed
                    },
                    Some(OperationError {
                        code: format!("compaction_{}", error.code),
                        message: error.message.clone(),
                    }),
                )
                .await;
                if aborted {
                    Ok(CompactionOutcome::DeclinedOrAborted {
                        leaf_id: self.leaf_id().await,
                    })
                } else {
                    Ok(CompactionOutcome::Failed {
                        leaf_id: self.leaf_id().await,
                        error: OperationError {
                            code: "compaction_failed".to_string(),
                            message: error.to_string(),
                        },
                    })
                }
            }
        }
    }

    pub async fn navigate_tree(
        &self,
        _target_id: Option<String>,
        _options: Option<NavigateOptions>,
    ) -> Result<NavigationOutcome, HarnessUnavailable> {
        self.unavailable("navigateTree")
    }

    /// 恢复:create 已把崩溃 operation 归约 aborted,正常运行后无挂起可续。
    pub async fn resume(&self) -> ResumeResult {
        if self.is_closed() {
            return Err(ResumeRejected::Closed(Closed::new(
                "AgentHarness was closed",
            )));
        }
        Err(ResumeRejected::NothingToResume(NothingToResume::new(
            "No suspended operation to resume; interrupted operations are restored as aborted by create().",
            "main".to_string(),
        )))
    }

    /// 中止当前 operation:先落 durable abort 标记,再拉 abort signal
    /// (对齐蓝本 durable abort 顺序);返回被清空队列的载荷。
    pub async fn abort(&self) -> AbortResult {
        let shared = self.shared();
        let handle = {
            let state = self.lock_state();
            if state.closed {
                return Err(AbortRejected::Closed(Closed::new(
                    "AgentHarness was closed",
                )));
            }
            let engine = state
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            engine
                .as_ref()
                .map(|engine| (engine.run_id.clone(), engine.signal.clone()))
        };
        let Some((run_id, signal)) = handle else {
            return Err(AbortRejected::NoActiveOperation(NoActiveOperation::new(
                "No active operation on lane main",
                "main".to_string(),
            )));
        };
        // 队列载荷收集 + 清空(abort 语义:返回给调用方自行处置)。
        let (steer, follow_up) = {
            let mut queues = shared
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                std::mem::take(&mut queues.steer),
                std::mem::take(&mut queues.follow_up),
            )
        };
        let _ = self
            .session
            .append_record(LaneRecord::AbortRequested(
                crate::agent::harness::session::types::AbortRequestedRecord {
                    id: uuid_v7(),
                    seq: 0,
                    lane: "main".to_string(),
                    timestamp: now_ms(),
                    run_id: run_id.clone(),
                },
            ))
            .await;
        signal.cancel();
        Ok(AbortOutcome {
            run_id,
            steer: steer.into_iter().map(|item| item.message).collect(),
            follow_up: follow_up.into_iter().map(|item| item.message).collect(),
        })
    }

    /// 入队公共实现:要求运行中,逐条消息写 QueueEnqueued 记录并进内存队列。
    async fn enqueue_to(&self, queue_kind: QueueKind, input: QueueInput) -> QueueResult {
        let shared = self.shared();
        let run_id = {
            let state = self.lock_state();
            if state.closed {
                return Err(QueueRejected::Closed(Closed::new(
                    "AgentHarness was closed",
                )));
            }
            let engine = state
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            engine.as_ref().map(|engine| engine.run_id.clone())
        };
        let Some(run_id) = run_id else {
            return Err(QueueRejected::NoActiveRun(NoActiveRun::new(
                "No active run; use prompt() to start one",
                "main".to_string(),
            )));
        };
        let messages: Vec<AgentMessage> = match input {
            QueueInput::Text(text) => vec![AgentMessage::user_text(text, now_ms())],
            QueueInput::Message(message) => vec![*message],
            QueueInput::Messages(messages) => messages,
        };
        if messages.is_empty() {
            return Err(QueueRejected::InvalidMessage(InvalidMessage::new(
                "Queue input must contain at least one message",
                "main".to_string(),
                "empty input".to_string(),
            )));
        }
        let mut last_id = String::new();
        for message in messages {
            let entry_id = uuid_v7();
            let record = QueueEnqueuedRecord {
                id: uuid_v7(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                queue: queue_kind,
                run_id: Some(run_id.clone()),
                target: ProvisionedEntry::Message(ProvisionedMessageEntry {
                    id: entry_id.clone(),
                    message: message.clone(),
                    terminate: None,
                }),
            };
            self.session
                .append_record(LaneRecord::QueueEnqueued(record))
                .await
                .map_err(|error| {
                    QueueRejected::InvalidMessage(InvalidMessage::new(
                        error.to_string(),
                        "main".to_string(),
                        format!("record({})", error.code),
                    ))
                })?;
            {
                let mut queues = shared
                    .queues
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let target = match queue_kind {
                    QueueKind::Steer => &mut queues.steer,
                    QueueKind::FollowUp => &mut queues.follow_up,
                    QueueKind::NextRun => &mut queues.next_run,
                };
                target.push(QueuedEntry {
                    entry_id: entry_id.clone(),
                    message,
                });
            }
            last_id = entry_id;
        }
        Ok(last_id)
    }

    pub async fn steer(&self, input: QueueInput) -> QueueResult {
        self.enqueue_to(QueueKind::Steer, input).await
    }

    pub async fn follow_up(&self, input: QueueInput) -> QueueResult {
        self.enqueue_to(QueueKind::FollowUp, input).await
    }

    pub async fn next_run(&self, input: QueueInput) -> QueueResult {
        self.enqueue_to(QueueKind::NextRun, input).await
    }

    pub async fn cancel_queued(&self, entry_id: String) -> CancelQueuedResult {
        let shared = self.shared();
        if self.is_closed() {
            return Err(CancelQueuedRejected::Closed(Closed::new(
                "AgentHarness was closed",
            )));
        }
        // 先从内存队列移除。
        let removed = {
            let mut queues = shared
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut removed = false;
            if let Some(position) = queues
                .steer
                .iter()
                .position(|item| item.entry_id == entry_id)
            {
                queues.steer.remove(position);
                removed = true;
            } else if let Some(position) = queues
                .follow_up
                .iter()
                .position(|item| item.entry_id == entry_id)
            {
                queues.follow_up.remove(position);
                removed = true;
            } else if let Some(position) = queues
                .next_run
                .iter()
                .position(|item| item.entry_id == entry_id)
            {
                queues.next_run.remove(position);
                removed = true;
            }
            removed
        };
        if removed {
            let run_id = {
                self.lock_state()
                    .engine
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                    .map(|engine| engine.run_id.clone())
            };
            let _ = self
                .session
                .append_record(LaneRecord::QueueCancelled(QueueCancelledRecord {
                    id: uuid_v7(),
                    seq: 0,
                    lane: "main".to_string(),
                    timestamp: now_ms(),
                    run_id,
                    entry_id: entry_id.clone(),
                }))
                .await;
            return Ok(CancelQueuedOutcome::Cancelled);
        }
        // 队列无此条目:查 enqueued 记录判定 consumed/cleared。
        let records = self
            .session
            .find_records(RecordQuery {
                record_type: Some("queue_enqueued".to_string()),
                ..Default::default()
            })
            .await
            .unwrap_or_default();
        let Some(record) = records.into_iter().find(|record| {
            matches!(record,
                LaneRecord::QueueEnqueued(enqueued)
                    if enqueued.target.id() == entry_id)
        }) else {
            return Err(CancelQueuedRejected::UnknownQueueItem(
                UnknownQueueItem::new(
                    format!("Unknown queue item: {entry_id}"),
                    "main".to_string(),
                    entry_id,
                ),
            ));
        };
        let record_run_id = match &record {
            LaneRecord::QueueEnqueued(enqueued) => enqueued.run_id.clone(),
            _ => None,
        };
        let run_finished = match &record_run_id {
            Some(record_run_id) => self
                .session
                .find_records(RecordQuery {
                    record_type: Some("operation_finished".to_string()),
                    run_id: Some(record_run_id.clone()),
                    limit: Some(1),
                    ..Default::default()
                })
                .await
                .map(|records| !records.is_empty())
                .unwrap_or(false),
            None => false,
        };
        Ok(if run_finished {
            CancelQueuedOutcome::AlreadyCleared
        } else {
            CancelQueuedOutcome::AlreadyConsumed
        })
    }

    pub async fn record_usage(
        &self,
        usage: Usage,
        options: Option<RecordUsageOptions>,
    ) -> Result<(), HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        let options = options.unwrap_or_default();
        let run_id = {
            self.lock_state()
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .map(|engine| engine.run_id.clone())
        };
        self.session
            .append_record(LaneRecord::Usage(UsageRecord {
                id: uuid_v7(),
                seq: 0,
                lane: "main".to_string(),
                timestamp: now_ms(),
                usage,
                cause: UsageCauseKind::Adjustment,
                run_id,
                entry_id: options.entry_id,
                attempt: None,
                stop_reason: None,
                tool_call_id: None,
                details: options.details,
            }))
            .await
            .map(|_| ())
            .map_err(|error| {
                HarnessNotImplemented::new(format!("recordUsage({})", error.code)).into()
            })
    }

    pub async fn wait_for_idle(&self) -> Result<(), HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        let mut receiver = {
            let state = self.lock_state();
            state.busy.subscribe()
        };
        while *receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
        Ok(())
    }

    pub async fn run_when_idle(
        &self,
        callback: Box<dyn FnOnce() + Send>,
    ) -> Result<(), HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        let mut harness_busy = {
            let state = self.lock_state();
            state.busy.subscribe()
        };
        if !*harness_busy.borrow_and_update() {
            callback();
            return Ok(());
        }
        tokio::spawn(async move {
            let mut receiver = harness_busy;
            while *receiver.borrow_and_update() {
                if receiver.changed().await.is_err() {
                    break;
                }
            }
            callback();
        });
        Ok(())
    }

    pub async fn peek_action(&self) -> Result<Option<ActionInfo>, HarnessUnavailable> {
        self.unavailable("peekAction")
    }

    pub async fn execute_action(&self) -> Result<Option<ActionInfo>, HarnessUnavailable> {
        self.unavailable("executeAction")
    }

    pub async fn run_to_completion(&self) -> Result<(), HarnessUnavailable> {
        self.wait_for_idle().await
    }

    pub async fn watch(&self) -> Result<WatchHandle<LaneSnapshot>, HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        // 快照同步捕获:先异步取 transcript,再进 bus.watch 的同步闭包。
        let transcript = branch_entries(&self.session).await.unwrap_or_default();
        let snapshot = LaneSnapshot {
            lane: "main".to_string(),
            transcript,
            leaf_id: Some(self.leaf_id().await),
            operation: self.main_lane_operation().await,
            queues: {
                let state = self.lock_state();
                let queues = state
                    .queues
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                snapshot_queues(&queues)
            },
            pending_writes: Vec::new(),
            faulted: false,
        };
        Ok(self.events.watch(move || snapshot))
    }

    pub async fn watch_session(&self) -> Result<WatchHandle<SessionSnapshot>, HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        let lane_snapshot = self.main_lane().await;
        let snapshot = SessionSnapshot {
            lanes: vec![(lane_snapshot, None)],
            faulted: false,
        };
        Ok(self.events.watch(move || snapshot))
    }

    pub async fn lane(&self, name: String) -> Result<Option<Lane>, HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        if name != "main" {
            return Ok(None);
        }
        Ok(Some(self.main_lane().await))
    }

    pub async fn create_lane(
        &self,
        _name: String,
        _at: Option<String>,
    ) -> Result<Lane, HarnessUnavailable> {
        self.unavailable("createLane")
    }

    pub async fn lanes(&self) -> Result<Vec<Lane>, HarnessUnavailable> {
        if self.is_closed() {
            return Err(HarnessClosed.into());
        }
        Ok(vec![self.main_lane().await])
    }

    // ----- getter/setter(可用) -----

    pub async fn get_model(&self) -> Model {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .model
            .clone()
    }

    pub async fn set_model(&self, model: Model) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .model = model;
    }

    pub async fn get_thinking_level(&self) -> ModelThinkingLevel {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .thinking_level
    }

    pub async fn set_thinking_level(&self, level: ModelThinkingLevel) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .thinking_level = level;
    }

    pub async fn get_active_tools(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active_tool_names
            .clone()
    }

    pub async fn set_active_tools(&self, names: Vec<String>) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .active_tool_names = names;
    }

    pub async fn get_tools(&self) -> Vec<crate::agent::harness::types::AgentHarnessTool> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .tools
            .clone()
    }

    pub async fn set_tools(
        &self,
        tools: Vec<crate::agent::harness::types::AgentHarnessTool>,
        active_names: Option<Vec<String>>,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active_tool_names =
            active_names.unwrap_or_else(|| tools.iter().map(|tool| tool.name.clone()).collect());
        state.tools = tools;
    }

    pub async fn get_resources(&self) -> AgentHarnessResources {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        AgentHarnessResources {
            skills: state.resources.skills.clone(),
            prompt_templates: state.resources.prompt_templates.clone(),
        }
    }

    pub async fn set_resources(&self, resources: AgentHarnessResources) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resources = resources;
    }

    pub async fn get_stream_options(&self) -> AgentHarnessStreamOptions {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stream_options
            .clone()
    }

    pub async fn set_stream_options(&self, options: AgentHarnessStreamOptions) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stream_options = options;
    }

    /// 应用流选项补丁:标量字段 Some 即覆盖;headers/metadata 的内层键值为
    /// None 表示删除该键,外层 None 表示清空全部(对齐蓝本 patch 语义)。
    pub async fn patch_stream_options(&self, patch: AgentHarnessStreamOptionsPatch) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let options = &mut state.stream_options;
        if patch.transport.is_some() {
            options.transport = patch.transport.clone();
        }
        if patch.timeout_ms.is_some() {
            options.timeout_ms = patch.timeout_ms;
        }
        if patch.max_retries.is_some() {
            options.max_retries = patch.max_retries;
        }
        if patch.max_retry_delay_ms.is_some() {
            options.max_retry_delay_ms = patch.max_retry_delay_ms;
        }
        if patch.cache_retention.is_some() {
            options.cache_retention = patch.cache_retention;
        }
        match patch.headers {
            Some(Some(map)) => {
                let target = options.headers.get_or_insert_with(HashMap::new);
                for (key, value) in map {
                    match value {
                        Some(value) => {
                            target.insert(key, value);
                        }
                        None => {
                            target.remove(&key);
                        }
                    }
                }
            }
            Some(None) => {
                options.headers = None;
            }
            None => {}
        }
        match patch.metadata {
            Some(Some(map)) => {
                let target = options.metadata.get_or_insert_with(HashMap::new);
                for (key, value) in map {
                    match value {
                        Some(value) => {
                            target.insert(key, value);
                        }
                        None => {
                            target.remove(&key);
                        }
                    }
                }
            }
            Some(None) => {
                options.metadata = None;
            }
            None => {}
        }
    }

    pub async fn get_retry_policy(&self) -> RetryPolicy {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retry_policy
    }

    pub async fn set_retry_policy(&self, policy: RetryPolicy) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retry_policy = policy;
    }

    pub async fn get_compaction_settings(&self) -> CompactionSettings {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .compaction_settings
    }

    pub async fn set_compaction_settings(&self, settings: CompactionSettings) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .compaction_settings = settings;
    }

    pub async fn get_steering_mode(&self) -> QueueMode {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .steering_mode
    }

    pub async fn set_steering_mode(&self, mode: QueueMode) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .steering_mode = mode;
    }

    pub async fn get_follow_up_mode(&self) -> QueueMode {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .follow_up_mode
    }

    pub async fn set_follow_up_mode(&self, mode: QueueMode) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .follow_up_mode = mode;
    }

    /// 关闭 harness;关闭后运行方法返回 HarnessClosed。活跃 run 的 abort
    /// signal 同步拉起,在途引擎尽快收尾。
    pub async fn close(&self) {
        let signal = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.closed = true;
            let engine = state
                .engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            engine.as_ref().map(|engine| engine.signal.clone())
        };
        if let Some(signal) = signal {
            signal.cancel();
        }
    }
}

/// prompt 启动时的配置快照(短临界区内克隆,不跨 await 持锁)。
struct PromptSnapshot {
    model: Model,
    thinking_level: ModelThinkingLevel,
    active_tool_names: Vec<String>,
    tools: Vec<crate::agent::harness::types::AgentHarnessTool>,
    tool_context: Option<Arc<dyn ToolContext>>,
    system_prompt: Option<String>,
    tool_execution: ToolExecutionMode,
    stream_options: AgentHarnessStreamOptions,
    retry_policy: RetryPolicy,
    stream_fn: crate::agent::types::StreamFn,
}

/// 无真实 assistant 消息时(如首响应前中止)的结果占位消息。
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

/// 队列快照(排队条目连同消息)。
fn snapshot_queues(queues: &QueueSet) -> QueuesSnapshot {
    fn map(items: &[QueuedEntry]) -> Vec<QueuedItem> {
        items
            .iter()
            .map(|item| QueuedItem {
                entry_id: item.entry_id.clone(),
                message: item.message.clone(),
            })
            .collect()
    }
    QueuesSnapshot {
        steer: map(&queues.steer),
        follow_up: map(&queues.follow_up),
        next_run: map(&queues.next_run),
    }
}

/// 队列输入(text 或完整消息)。
pub enum QueueInput {
    Text(String),
    Message(Box<AgentMessage>),
    Messages(Vec<AgentMessage>),
}

/// compaction 选项(对齐 TS `{ customInstructions? }`)。
#[derive(Clone, Debug, Default)]
pub struct CompactOptions {
    pub custom_instructions: Option<String>,
}

/// recordUsage 选项。
#[derive(Clone, Debug, Default)]
pub struct RecordUsageOptions {
    pub entry_id: Option<String>,
    pub details: Option<crate::agent::harness::session::types::JsonValue>,
}

/// 动作信息(对齐 TS `ActionInfo`;字段按 kind 携带)。
#[derive(Clone, Debug)]
pub enum ActionInfo {
    AppendEntry {
        entry_type: String,
        entry_id: String,
    },
    AppendRecord {
        record_type: String,
    },
    MoveLane {
        to: Option<String>,
    },
    SetFact {
        fact: String,
    },
    TryFinishRun {
        outcome: String,
    },
    FinishOperation {
        outcome: String,
    },
    CommitFollowUp,
    ConsumeQueueItem {
        queue: String,
        entry_id: String,
    },
    ApplyPendingWrite {
        entry_id: String,
    },
    StreamAssistant {
        step: String,
        attempt: i64,
    },
    ExecuteTool {
        tool_call_id: String,
        tool_name: String,
    },
    DeferredFetch {
        provider: String,
        id: String,
    },
    Hook {
        name: HookName,
    },
    Sleep {
        delay_ms: i64,
    },
}
