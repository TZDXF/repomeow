//! AgentHarness 组合层:对齐 `packages/agent/src/harness/agent-harness.ts`。
//!
//! 蓝本在 0.84.4 为显式 scaffold;本仓库在保持其公开契约(类型/错误/结果形状)
//! 的前提下接入了运行时(runtime.rs):prompt/abort/steer/followUp/nextRun/
//! cancelQueued/recordUsage/waitForIdle/runWhenIdle/runToCompletion/watch/
//! watchSession/lane/lanes/compact 可用;resume 在 create 恢复后无挂起 operation
//! 可续(崩溃 operation 已归约 aborted),返回 `NothingToResume`。
//! 仍按蓝本 NotImplemented 的:skill/promptFromTemplate(资源未接线)、
//! navigateTree、peekAction/executeAction(manual drive)、createLane(单 lane)。
//!
//! 本文件只保留公开契约类型、状态容器(`HarnessState`)与队列/观测/存取方法;
//! 运行逻辑按职责拆到兄弟模块(同一类型上的分块 impl,跨模块辅助为 pub(crate)):
//! - `run.rs`:prompt 运行管线(prepare → context → engine → drive → finalize);
//! - `compaction_runner.rs`:compact/auto_compact 压缩执行;
//! - `lifecycle.rs`:create 的崩溃恢复归约、resume、abort。
//! 上层消费方:`commands/ai/harness_support.rs`(wiki 生成与资源库安全扫描共用)。

use crate::agent::agent_loop::now_ms;
use crate::agent::harness::compaction::compaction::CompactionSettings;
use crate::agent::harness::compaction_runner::CompactSnapshot;
use crate::agent::harness::context::Context as HarnessContext;
use crate::agent::harness::errors::{
    Closed, HarnessClosed, HarnessNotImplemented, HarnessUnavailable, InvalidLane, InvalidMessage,
    LaneBusy, LaneExists, MissingIdentities, NoActiveOperation, NoActiveRun, NothingToCompact,
    NothingToResume, OperationError, UnknownQueueItem, UnknownSkill, UnknownTarget,
    UnknownTemplate,
};
use crate::agent::harness::events::{
    HarnessEvent, HarnessEventBus, HarnessEventListener, HarnessEventType, WatchHandle,
};
use crate::agent::harness::execution::{create_gate, GateControl};
use crate::agent::harness::hooks::{
    BeforeDriveEvent, BeforeNavigationEvent, DriveOperation, HookEvent, HookName, HookRegistry,
    HookResult,
};
use crate::agent::harness::run::PromptSnapshot;
use crate::agent::harness::runtime::{
    branch_entries, EngineHandle, QueueSet, QueuedEntry, RuntimeShared,
};
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    BranchSummaryEntry, CompactionEntry, Entry, LaneRecord, OperationOutcome, ProvisionedEntry,
    ProvisionedMessageEntry, QueueCancelledRecord, QueueEnqueuedRecord, QueueKind, RecordQuery,
    SessionError, SessionTree, UsageCauseKind, UsageRecord,
};
use crate::agent::harness::telemetry::TelemetryContext;
use crate::agent::harness::types::{
    AgentHarnessResources, AgentHarnessStreamOptions, AgentHarnessStreamOptionsPatch,
    Result as ResultValue, ToolContext,
};
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::types::{AssistantMessage, Model, ModelThinkingLevel, ThinkingLevel, Usage};
use crate::agent::types::{AgentMessage, QueueMode, StreamFn, ToolExecutionMode};
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
    pub hooks: HookRegistry,
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

pub(crate) struct HarnessState {
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
    hooks: Arc<HookRegistry>,
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
            hooks,
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
                    hooks: Arc::new(hooks),
                    queues,
                    engine,
                    busy,
                }),
            },
            suspended,
        ))
    }

    fn unavailable<T>(&self, operation: &str) -> Result<T, HarnessUnavailable> {
        if self.is_closed() {
            Err(HarnessClosed.into())
        } else {
            Err(HarnessNotImplemented::new(operation).into())
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
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
    pub(crate) fn shared(&self) -> RuntimeShared {
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

    pub(crate) fn lock_state(&self) -> std::sync::MutexGuard<'_, HarnessState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn closed_error() -> RunRejected {
        RunRejected::Closed(Closed::new("AgentHarness was closed"))
    }

    pub(crate) async fn leaf_id(&self) -> String {
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
    pub(crate) async fn finish_run(
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
    /// prompt 守卫 + 配置快照(短临界区,不跨 await):closed/LaneBusy 检查
    /// 通过后在同一临界区克隆运行所需全部配置。
    pub(crate) fn guard_and_snapshot(&self) -> Result<PromptSnapshot, RunRejected> {
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
        Ok(PromptSnapshot {
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
            resources: AgentHarnessResources {
                prompt_templates: state.resources.prompt_templates.clone(),
                skills: state.resources.skills.clone(),
            },
            hooks: state.hooks.clone(),
            telemetry_context: state.telemetry_context.clone(),
        })
    }

    /// compact 守卫 + 配置快照(单临界区):closed → HarnessClosed;
    /// 引擎忙 → compact.busy(检查顺序与拆分前 compact 开头一致)。
    pub(crate) fn guard_compact(&self) -> Result<CompactSnapshot, HarnessUnavailable> {
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
        Ok(CompactSnapshot {
            settings: state.compaction_settings,
            model: state.model.clone(),
            thinking_level: crate::agent::agent_loop::reasoning_from_thinking_level(
                state.thinking_level,
            ),
            stream_fn: state
                .stream_fn
                .clone()
                .expect("stream_fn is set at create time"),
            hooks: state.hooks.clone(),
            telemetry_context: state.telemetry_context.clone(),
        })
    }

    /// auto_compact 配置快照(单临界区):closed 或压缩未启用时返回 None。
    pub(crate) fn auto_compact_config(
        &self,
    ) -> Option<(CompactionSettings, Model, Option<ThinkingLevel>, StreamFn)> {
        let state = self.lock_state();
        if state.closed || !state.compaction_settings.enabled {
            return None;
        }
        Some((
            state.compaction_settings,
            state.model.clone(),
            crate::agent::agent_loop::reasoning_from_thinking_level(state.thinking_level),
            state
                .stream_fn
                .clone()
                .expect("stream_fn is set at create time"),
        ))
    }

    /// abort 守卫(单临界区):closed → Closed;无在途引擎 → NoActiveOperation;
    /// 否则取出 (run_id, signal, gate_control)。
    pub(crate) fn guard_abort(
        &self,
    ) -> Result<(String, tokio_util::sync::CancellationToken, GateControl), AbortRejected> {
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
            .map(|engine| {
                (
                    engine.run_id.clone(),
                    engine.signal.clone(),
                    engine.gate_control.clone(),
                )
            })
            .ok_or_else(|| {
                AbortRejected::NoActiveOperation(NoActiveOperation::new(
                    "No active operation on lane main",
                    "main".to_string(),
                ))
            })
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

    pub async fn navigate_tree(
        &self,
        target_id: Option<String>,
        options: Option<NavigateOptions>,
    ) -> Result<NavigationOutcome, HarnessUnavailable> {
        let (hooks, telemetry_context) = {
            let state = self.lock_state();
            (state.hooks.clone(), state.telemetry_context.clone())
        };
        let run_id = uuid_v7();
        let (gate, _control) = create_gate();
        let context = HarnessContext::new()
            .with_telemetry_opt(telemetry_context)
            .with_signal(gate.signal());
        if hooks
            .run_with_gate(
                HookName::BeforeDrive,
                HookEvent::BeforeDrive(BeforeDriveEvent {
                    lane: "main".to_string(),
                    run_id: run_id.clone(),
                    operation: DriveOperation::Navigation,
                }),
                &gate,
                context.clone(),
            )
            .await
            .is_err()
        {
            return Ok(NavigationOutcome::Failed {
                leaf_id: Some(self.leaf_id().await),
                error: OperationError {
                    code: "hook_gate".to_string(),
                    message: "Hook gate rejected navigation".to_string(),
                },
            });
        }
        let hook = hooks
            .run_with_gate(
                HookName::BeforeNavigation,
                HookEvent::BeforeNavigation(BeforeNavigationEvent {
                    lane: "main".to_string(),
                    run_id,
                    target_id: target_id.unwrap_or_default(),
                    preparation: serde_json::json!({}),
                    custom_instructions: options.and_then(|options| options.custom_instructions),
                }),
                &gate,
                context,
            )
            .await;
        if let Ok(Some(HookResult::BeforeNavigation(Some(result)))) = hook {
            if result.decline == Some(true) {
                return Ok(NavigationOutcome::DeclinedOrAborted {
                    leaf_id: Some(self.leaf_id().await),
                });
            }
        } else if hook.is_err() {
            return Ok(NavigationOutcome::Failed {
                leaf_id: Some(self.leaf_id().await),
                error: OperationError {
                    code: "hook_gate".to_string(),
                    message: "Hook gate rejected navigation".to_string(),
                },
            });
        }
        self.unavailable("navigateTree")
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

    /// 返回 11 类 hook 的注册数量。
    pub fn hook_counts(&self) -> [usize; 11] {
        self.lock_state().hooks.counts()
    }

    /// 当前 hook registry 快照(注册/取消订阅入口)。
    pub fn hooks(&self) -> Arc<HookRegistry> {
        self.lock_state().hooks.clone()
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
