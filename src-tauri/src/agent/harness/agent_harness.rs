//! AgentHarness 组合层:对齐 `packages/agent/src/harness/agent-harness.ts`。
//!
//! 与蓝本一致,`AgentHarness` 在上游即为 WIP:运行方法(prompt/skill/compact/
//! navigate/resume/abort/steer/followUp/nextRun/watch 等)全部返回
//! [`HarnessNotImplemented`](关闭后返回 [`HarnessClosed`]);getter/setter 可用。
//! 错误族定义在 `errors.rs`,结果类型别名与蓝本一一对应。

use crate::agent::harness::compaction::compaction::CompactionSettings;
use crate::agent::harness::errors::{
    Closed, HarnessClosed, HarnessNotImplemented, HarnessUnavailable, InvalidLane, InvalidMessage,
    LaneBusy, LaneExists, MissingIdentities, NoActiveOperation, NoActiveRun, NothingToCompact,
    NothingToResume, OperationError, UnknownQueueItem, UnknownSkill, UnknownTarget, UnknownTemplate,
};
use crate::agent::harness::events::{HarnessEventBus, HarnessEvent, HarnessEventType, WatchHandle};
use crate::agent::harness::events::HarnessEventListener;
use crate::agent::harness::types::Result as ResultValue;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    BranchSummaryEntry, CompactionEntry, Entry, ProvisionedEntry, RecordQuery, SessionError,
    SessionTree,
};
use crate::agent::harness::telemetry::TelemetryContext;
use crate::agent::harness::types::{
    AgentHarnessResources, AgentHarnessStreamOptions, AgentHarnessStreamOptionsPatch,
};
use crate::agent::llm::types::{AssistantMessage, Model, ModelThinkingLevel, Usage};
use crate::agent::types::{AgentMessage, QueueMode, ToolExecutionMode};
use std::sync::Mutex;

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
// AgentHarness(WIP 骨架)
// ---------------------------------------------------------------------------

/// AgentHarness:与蓝本相同的 WIP 形态 —— 字段访问器可用,运行方法返回
/// `HarnessNotImplemented`(关闭后 `HarnessClosed`)。
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
}

impl AgentHarness {
    /// 创建 harness(对齐 TS `create`;存在任何历史记录时抛 NotImplemented)。
    pub async fn create(
        options: AgentHarnessOptions,
    ) -> Result<(Self, Vec<SuspendedOperation>), HarnessNotImplemented> {
        let record = options
            .session
            .find_records(RecordQuery {
                limit: Some(1),
                ..Default::default()
            })
            .await
            .map_err(|error: SessionError| {
                HarnessNotImplemented::new(format!("create.restore({})", error.code))
            })?;
        if !record.is_empty() {
            return Err(HarnessNotImplemented::new("create.restore"));
        }
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
        // WIP:运行期字段尚未接入驱动循环,与蓝本构造器一样仅存档。
        let _ = (
            stream_fn,
            tool_context,
            system_prompt,
            tool_execution,
            telemetry_context,
        );
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
                    compaction_settings: compaction
                        .unwrap_or(crate::agent::harness::compaction::compaction::DEFAULT_COMPACTION_SETTINGS),
                    steering_mode,
                    follow_up_mode,
                    closed: false,
                }),
            },
            Vec::new(),
        ))
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

    /// 事件总线(hooks.on / events.on 在蓝本同样抛 NotImplemented;事件经
    /// run_start/run_end 预留,见 events.rs)。
    pub fn events(&self) -> &HarnessEventBus {
        &self.events
    }

    /// 注册 run_start 事件监听(WIP:事件不会发出,但注册本身可用)。
    pub fn on_event(&self, listener: HarnessEventListener) -> Box<dyn FnOnce() + Send> {
        self.events.on(HarnessEventType::RunStart, listener)
    }

    pub fn emit_event(&self, event: &HarnessEvent) {
        self.events.emit(event);
    }

    pub async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.session.get_leaf_id().await
    }

    pub async fn prompt(&self, _text: String) -> Result<RunOutcome, HarnessUnavailable> {
        self.unavailable("prompt")
    }

    pub async fn skill(&self, _name: String, _additional_instructions: Option<String>) -> Result<RunOutcome, HarnessUnavailable> {
        self.unavailable("skill")
    }

    pub async fn prompt_from_template(
        &self,
        _name: String,
        _args: Option<Vec<String>>,
    ) -> Result<RunOutcome, HarnessUnavailable> {
        self.unavailable("promptFromTemplate")
    }

    pub async fn compact(&self, _options: Option<CompactOptions>) -> Result<CompactionOutcome, HarnessUnavailable> {
        self.unavailable("compact")
    }

    pub async fn navigate_tree(
        &self,
        _target_id: Option<String>,
        _options: Option<NavigateOptions>,
    ) -> Result<NavigationOutcome, HarnessUnavailable> {
        self.unavailable("navigateTree")
    }

    pub async fn resume(&self) -> Result<ResumeOutcome, HarnessUnavailable> {
        self.unavailable("resume")
    }

    pub async fn abort(&self) -> Result<AbortOutcome, HarnessUnavailable> {
        self.unavailable("abort")
    }

    pub async fn steer(&self, _input: QueueInput) -> Result<String, HarnessUnavailable> {
        self.unavailable("steer")
    }

    pub async fn follow_up(&self, _input: QueueInput) -> Result<String, HarnessUnavailable> {
        self.unavailable("followUp")
    }

    pub async fn next_run(&self, _input: QueueInput) -> Result<String, HarnessUnavailable> {
        self.unavailable("nextRun")
    }

    pub async fn cancel_queued(&self, _entry_id: String) -> Result<CancelQueuedOutcome, HarnessUnavailable> {
        self.unavailable("cancelQueued")
    }

    pub async fn record_usage(
        &self,
        _usage: Usage,
        _options: Option<RecordUsageOptions>,
    ) -> Result<(), HarnessUnavailable> {
        self.unavailable("recordUsage")
    }

    pub async fn wait_for_idle(&self) -> Result<(), HarnessUnavailable> {
        self.unavailable("waitForIdle")
    }

    pub async fn run_when_idle(
        &self,
        _callback: Box<dyn FnOnce() + Send>,
    ) -> Result<(), HarnessUnavailable> {
        self.unavailable("runWhenIdle")
    }

    pub async fn peek_action(&self) -> Result<Option<ActionInfo>, HarnessUnavailable> {
        self.unavailable("peekAction")
    }

    pub async fn execute_action(&self) -> Result<Option<ActionInfo>, HarnessUnavailable> {
        self.unavailable("executeAction")
    }

    pub async fn run_to_completion(&self) -> Result<(), HarnessUnavailable> {
        self.unavailable("runToCompletion")
    }

    pub async fn watch(&self) -> Result<WatchHandle<LaneSnapshot>, HarnessUnavailable> {
        self.unavailable("watch")
    }

    pub async fn watch_session(&self) -> Result<WatchHandle<SessionSnapshot>, HarnessUnavailable> {
        self.unavailable("watchSession")
    }

    pub async fn lane(&self, _name: String) -> Result<Option<Lane>, HarnessUnavailable> {
        self.unavailable("lane")
    }

    pub async fn create_lane(&self, _name: String, _at: Option<String>) -> Result<Lane, HarnessUnavailable> {
        self.unavailable("createLane")
    }

    pub async fn lanes(&self) -> Result<Vec<Lane>, HarnessUnavailable> {
        self.unavailable("lanes")
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
        state.active_tool_names = active_names
            .unwrap_or_else(|| tools.iter().map(|tool| tool.name.clone()).collect());
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

    pub async fn patch_stream_options(&self, _patch: AgentHarnessStreamOptionsPatch) {
        // WIP:补丁应用语义与蓝本一致,但 harness 运行方法未实现,先保持无操作。
        let _ = &self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    /// 关闭 harness;关闭后运行方法返回 HarnessClosed。
    pub async fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .closed = true;
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
