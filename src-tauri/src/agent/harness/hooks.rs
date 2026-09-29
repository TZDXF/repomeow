//! harness 生命周期 hooks:对齐 `packages/agent/src/harness/hooks.ts` 的
//! 11 类生命周期、注册表聚合与错误语义。Rust 侧使用 owned event 与
//! `BoxFuture<'static>`;handler 按注册顺序串行执行。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::Value;

use crate::agent::harness::context::Context;
use crate::agent::harness::execution::Gate;
use crate::agent::harness::telemetry::{start_harness_hook_span, TelemetryContext, NOOP};
use crate::agent::llm::types::{AssistantMessage, TextOrImageContent, Usage};
use crate::agent::types::AgentMessage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveOperation {
    Run,
    Compaction,
    Navigation,
}

impl DriveOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Compaction => "compaction",
            Self::Navigation => "navigation",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestStep {
    Assistant,
    Deferred,
    Compaction,
    BranchSummary,
}

impl RequestStep {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Deferred => "deferred",
            Self::Compaction => "compaction",
            Self::BranchSummary => "branch_summary",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookCompactionReason {
    Manual,
    Threshold,
    Overflow,
}

impl HookCompactionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Threshold => "threshold",
            Self::Overflow => "overflow",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookName {
    BeforeRun,
    BeforeDrive,
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

pub const ALL_HOOK_NAMES: [HookName; 11] = [
    HookName::BeforeRun,
    HookName::BeforeDrive,
    HookName::BeforeRunEnd,
    HookName::TransformContext,
    HookName::BeforeRequest,
    HookName::BeforePayload,
    HookName::AfterResponse,
    HookName::BeforeTool,
    HookName::AfterTool,
    HookName::BeforeCompaction,
    HookName::BeforeNavigation,
];

impl HookName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BeforeRun => "before_run",
            Self::BeforeDrive => "before_drive",
            Self::BeforeRunEnd => "before_run_end",
            Self::TransformContext => "transform_context",
            Self::BeforeRequest => "before_request",
            Self::BeforePayload => "before_payload",
            Self::AfterResponse => "after_response",
            Self::BeforeTool => "before_tool",
            Self::AfterTool => "after_tool",
            Self::BeforeCompaction => "before_compaction",
            Self::BeforeNavigation => "before_navigation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct HookError {
    pub kind: HookErrorKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookErrorKind {
    Handler,
    GateAborting,
    GateClosed,
    Cancelled,
}

impl HookError {
    pub fn handler(message: impl Into<String>) -> Self {
        Self {
            kind: HookErrorKind::Handler,
            message: message.into(),
        }
    }
    pub fn aborting() -> Self {
        Self {
            kind: HookErrorKind::GateAborting,
            message: "Abort requested".to_string(),
        }
    }
    pub fn closed(message: impl Into<String>) -> Self {
        Self {
            kind: HookErrorKind::GateClosed,
            message: message.into(),
        }
    }
    pub fn cancelled() -> Self {
        Self {
            kind: HookErrorKind::Cancelled,
            message: "Operation cancelled".to_string(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct BeforeRunEvent {
    pub lane: String,
    pub run_id: String,
    pub prompt: Vec<AgentMessage>,
    pub resources: Value,
}
#[derive(Clone, Debug)]
pub struct BeforeDriveEvent {
    pub lane: String,
    pub run_id: String,
    pub operation: DriveOperation,
}
#[derive(Clone, Debug)]
pub struct BeforeRunEndEvent {
    pub lane: String,
    pub run_id: String,
    pub messages: Vec<AgentMessage>,
}
#[derive(Clone, Debug)]
pub struct TransformContextEvent {
    pub lane: String,
    pub run_id: String,
    pub messages: Vec<AgentMessage>,
    pub system_prompt: String,
}
#[derive(Clone, Debug)]
pub struct BeforeRequestEvent {
    pub lane: String,
    pub run_id: String,
    pub model: String,
    pub step: RequestStep,
    pub attempt: u32,
    pub stream_options: Value,
}
#[derive(Clone, Debug)]
pub struct BeforePayloadEvent {
    pub lane: String,
    pub run_id: String,
    pub model: String,
    pub payload: Value,
}
#[derive(Clone, Debug)]
pub struct AfterResponseEvent {
    pub lane: String,
    pub run_id: String,
    pub status: Option<u16>,
    pub headers: Option<HashMap<String, String>>,
    pub message: AssistantMessage,
}
#[derive(Clone, Debug)]
pub struct BeforeToolEvent {
    pub lane: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
}
#[derive(Clone, Debug)]
pub struct AfterToolEvent {
    pub lane: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub content: Vec<TextOrImageContent>,
    pub details: Option<Value>,
    pub is_error: bool,
    pub usage: Option<Usage>,
}
#[derive(Clone, Debug)]
pub struct BeforeCompactionEvent {
    pub lane: String,
    pub run_id: String,
    pub reason: HookCompactionReason,
    pub preparation: Value,
    pub custom_instructions: Option<String>,
}
#[derive(Clone, Debug)]
pub struct BeforeNavigationEvent {
    pub lane: String,
    pub run_id: String,
    pub target_id: String,
    pub preparation: Value,
    pub custom_instructions: Option<String>,
}

#[derive(Clone, Debug)]
pub enum HookEvent {
    BeforeRun(BeforeRunEvent),
    BeforeDrive(BeforeDriveEvent),
    BeforeRunEnd(BeforeRunEndEvent),
    TransformContext(TransformContextEvent),
    BeforeRequest(BeforeRequestEvent),
    BeforePayload(BeforePayloadEvent),
    AfterResponse(AfterResponseEvent),
    BeforeTool(BeforeToolEvent),
    AfterTool(AfterToolEvent),
    BeforeCompaction(BeforeCompactionEvent),
    BeforeNavigation(BeforeNavigationEvent),
}

impl HookEvent {
    fn lane(&self) -> &str {
        match self {
            Self::BeforeRun(v) => &v.lane,
            Self::BeforeDrive(v) => &v.lane,
            Self::BeforeRunEnd(v) => &v.lane,
            Self::TransformContext(v) => &v.lane,
            Self::BeforeRequest(v) => &v.lane,
            Self::BeforePayload(v) => &v.lane,
            Self::AfterResponse(v) => &v.lane,
            Self::BeforeTool(v) => &v.lane,
            Self::AfterTool(v) => &v.lane,
            Self::BeforeCompaction(v) => &v.lane,
            Self::BeforeNavigation(v) => &v.lane,
        }
    }
    fn run_id(&self) -> &str {
        match self {
            Self::BeforeRun(v) => &v.run_id,
            Self::BeforeDrive(v) => &v.run_id,
            Self::BeforeRunEnd(v) => &v.run_id,
            Self::TransformContext(v) => &v.run_id,
            Self::BeforeRequest(v) => &v.run_id,
            Self::BeforePayload(v) => &v.run_id,
            Self::AfterResponse(v) => &v.run_id,
            Self::BeforeTool(v) => &v.run_id,
            Self::AfterTool(v) => &v.run_id,
            Self::BeforeCompaction(v) => &v.run_id,
            Self::BeforeNavigation(v) => &v.run_id,
        }
    }
    fn hook_name(&self) -> HookName {
        match self {
            Self::BeforeRun(_) => HookName::BeforeRun,
            Self::BeforeDrive(_) => HookName::BeforeDrive,
            Self::BeforeRunEnd(_) => HookName::BeforeRunEnd,
            Self::TransformContext(_) => HookName::TransformContext,
            Self::BeforeRequest(_) => HookName::BeforeRequest,
            Self::BeforePayload(_) => HookName::BeforePayload,
            Self::AfterResponse(_) => HookName::AfterResponse,
            Self::BeforeTool(_) => HookName::BeforeTool,
            Self::AfterTool(_) => HookName::AfterTool,
            Self::BeforeCompaction(_) => HookName::BeforeCompaction,
            Self::BeforeNavigation(_) => HookName::BeforeNavigation,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BeforeRunResult {
    pub messages: Option<Vec<AgentMessage>>,
}
#[derive(Clone, Debug)]
pub struct BeforeRunEndResult {
    pub follow_up: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct TransformContextResult {
    pub messages: Option<Vec<AgentMessage>>,
    pub system_prompt: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct BeforeRequestResult {
    pub stream_options: Option<Value>,
}
#[derive(Clone, Debug)]
pub struct BeforePayloadResult {
    pub payload: Value,
}
#[derive(Clone, Debug)]
pub struct AfterResponseResult {
    pub message: Option<AssistantMessage>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolBlock {
    pub reason: String,
    pub terminate: Option<bool>,
}
#[derive(Clone, Debug, Default)]
pub struct BeforeToolResult {
    pub args: Option<Value>,
    pub block: Option<ToolBlock>,
}
#[derive(Clone, Debug, Default)]
pub struct AfterToolResult {
    pub content: Option<Vec<TextOrImageContent>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Usage>,
    pub terminate: Option<bool>,
}
#[derive(Clone, Debug, Default)]
pub struct BeforeCompactionResult {
    pub decline: Option<bool>,
    pub compaction: Option<Value>,
}
#[derive(Clone, Debug, Default)]
pub struct BeforeNavigationResult {
    pub decline: Option<bool>,
    pub summary: Option<Value>,
}

#[derive(Clone, Debug)]
pub enum HookResult {
    Unit,
    BeforeRun(Option<BeforeRunResult>),
    BeforeRunEnd(Option<BeforeRunEndResult>),
    TransformContext(Option<TransformContextResult>),
    BeforeRequest(Option<BeforeRequestResult>),
    BeforePayload(Option<BeforePayloadResult>),
    AfterResponse(Option<AfterResponseResult>),
    BeforeTool(Option<BeforeToolResult>),
    AfterTool(Option<AfterToolResult>),
    BeforeCompaction(Option<BeforeCompactionResult>),
    BeforeNavigation(Option<BeforeNavigationResult>),
}

pub type HookHandler = Arc<
    dyn Fn(HookEvent, Context) -> BoxFuture<'static, Result<HookResult, HookError>> + Send + Sync,
>;
pub type HookErrorReporter =
    Arc<dyn Fn(HookError, HookName, String, Context) -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Clone)]
pub struct HookRegistration {
    pub id: Option<String>,
    pub sequence: u64,
    pub handler: HookHandler,
}

struct RegistryState {
    registrations: HashMap<HookName, Vec<HookRegistration>>,
    closed: Option<String>,
    next_sequence: u64,
}
#[derive(Clone)]
pub struct HookRegistry {
    state: Arc<Mutex<RegistryState>>,
    report_error: HookErrorReporter,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::with_reporter(Arc::new(|_, _, _, _| Box::pin(async {})))
    }
}

impl std::fmt::Debug for HookRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRegistry")
            .field("counts", &self.counts())
            .field("closed", &self.closed_error())
            .finish()
    }
}

#[derive(Clone)]
pub struct HookUnsubscribe {
    registry: HookRegistry,
    name: HookName,
    sequence: u64,
}

impl HookUnsubscribe {
    pub fn unsubscribe(&self) {
        let mut state = self.registry.lock();
        if let Some(registrations) = state.registrations.get_mut(&self.name) {
            registrations.retain(|registration| registration.sequence != self.sequence);
        }
    }
}

impl HookRegistry {
    pub fn with_reporter(report_error: HookErrorReporter) -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState {
                registrations: HashMap::new(),
                closed: None,
                next_sequence: 0,
            })),
            report_error,
        }
    }

    pub fn on(
        &self,
        name: HookName,
        handler: HookHandler,
        id: Option<String>,
    ) -> Result<HookUnsubscribe, HookError> {
        let mut state = self.lock();
        if let Some(error) = state.closed.clone() {
            return Err(HookError::closed(error));
        }
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        state
            .registrations
            .entry(name)
            .or_default()
            .push(HookRegistration {
                id,
                sequence,
                handler,
            });
        Ok(HookUnsubscribe {
            registry: self.clone(),
            name,
            sequence,
        })
    }

    pub fn has(&self, name: HookName) -> bool {
        self.count(name) != 0
    }
    pub fn count(&self, name: HookName) -> usize {
        self.lock().registrations.get(&name).map_or(0, Vec::len)
    }

    pub fn counts(&self) -> [usize; 11] {
        let state = self.lock();
        let mut counts = [0_usize; 11];
        for (index, name) in ALL_HOOK_NAMES.iter().enumerate() {
            counts[index] = state.registrations.get(name).map_or(0, Vec::len);
        }
        counts
    }

    pub fn close(&self, error: impl Into<String>) {
        let mut state = self.lock();
        if state.closed.is_none() {
            state.closed = Some(error.into());
        }
    }

    pub fn closed_error(&self) -> Option<String> {
        self.lock().closed.clone()
    }

    pub async fn run_with_gate(
        &self,
        name: HookName,
        event: HookEvent,
        gate: &Gate,
        context: Context,
    ) -> Result<Option<HookResult>, HookError> {
        let registry = self.clone();
        let event = event.clone();
        let context = context.clone();
        gate.admit(|| async move { registry.run_admitted(name, event, context).await })
            .await
    }

    pub async fn run_tool_with_gate(
        &self,
        name: HookName,
        event: HookEvent,
        gate: &Gate,
        context: Context,
    ) -> Result<Option<HookResult>, HookError> {
        if !matches!(name, HookName::BeforeTool | HookName::AfterTool) {
            return Err(HookError::handler(
                "run_tool_with_gate accepts only before_tool or after_tool",
            ));
        }
        let registry = self.clone();
        gate.admit(|| async move {
            if context.is_cancelled() {
                return Err(HookError::cancelled());
            }
            match name {
                HookName::BeforeTool => registry
                    .before_tool(expect_before_tool(event, name)?, context)
                    .await
                    .map(Some),
                HookName::AfterTool => registry
                    .after_tool(expect_after_tool(event, name)?, context)
                    .await
                    .map(Some),
                _ => unreachable!("checked above"),
            }
        })
        .await
    }

    async fn run_admitted(
        &self,
        name: HookName,
        event: HookEvent,
        context: Context,
    ) -> Result<Option<HookResult>, HookError> {
        if let Some(error) = self.closed_error() {
            return Err(HookError::closed(error));
        }
        if context.is_cancelled() {
            return Err(HookError::cancelled());
        }
        if event.hook_name() != name {
            return Err(HookError::handler("hook event does not match hook name"));
        }
        match name {
            HookName::BeforeRun => self
                .before_run(expect_before_run(event, name)?, context)
                .await
                .map(Some),
            HookName::BeforeDrive => {
                self.before_drive(expect_before_drive(event, name)?, context)
                    .await?;
                Ok(Some(HookResult::Unit))
            }
            HookName::BeforeRunEnd => self
                .before_run_end(expect_before_run_end(event, name)?, context)
                .await
                .map(Some),
            HookName::TransformContext => self
                .transform_context(expect_transform_context(event, name)?, context)
                .await
                .map(Some),
            HookName::BeforeRequest => self
                .before_request(expect_before_request(event, name)?, context)
                .await
                .map(Some),
            HookName::BeforePayload => self
                .before_payload(expect_before_payload(event, name)?, context)
                .await
                .map(Some),
            HookName::AfterResponse => self
                .after_response(expect_after_response(event, name)?, context)
                .await
                .map(Some),
            HookName::BeforeTool => self
                .before_tool(expect_before_tool(event, name)?, context)
                .await
                .map(Some),
            HookName::AfterTool => self
                .after_tool(expect_after_tool(event, name)?, context)
                .await
                .map(Some),
            HookName::BeforeCompaction => self
                .before_compaction(expect_before_compaction(event, name)?, context)
                .await
                .map(Some),
            HookName::BeforeNavigation => self
                .before_navigation(expect_before_navigation(event, name)?, context)
                .await
                .map(Some),
        }
    }
    async fn before_run(
        &self,
        mut event: BeforeRunEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        let mut injected = Vec::new();
        for registration in self.registrations_for(HookName::BeforeRun) {
            match invoke_handler(
                &registration,
                HookEvent::BeforeRun(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::BeforeRun(result)) => {
                    if let Some(messages) = result.and_then(|v| v.messages) {
                        injected.extend(messages.iter().cloned());
                        event.prompt.extend(messages);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::BeforeRun,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(if injected.is_empty() {
            HookResult::BeforeRun(None)
        } else {
            HookResult::BeforeRun(Some(BeforeRunResult {
                messages: Some(injected),
            }))
        })
    }

    async fn before_drive(
        &self,
        event: BeforeDriveEvent,
        context: Context,
    ) -> Result<(), HookError> {
        for registration in self.registrations_for(HookName::BeforeDrive) {
            if let Err(mut error) = invoke_handler(
                &registration,
                HookEvent::BeforeDrive(event.clone()),
                context.clone(),
            )
            .await
            {
                error.kind = HookErrorKind::Handler;
                self.report(
                    error.clone(),
                    HookName::BeforeDrive,
                    event.lane.clone(),
                    context.clone(),
                )
                .await;
                return Err(error);
            }
        }
        Ok(())
    }

    async fn before_run_end(
        &self,
        event: BeforeRunEndEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        let mut follow_up = None;
        for registration in self.registrations_for(HookName::BeforeRunEnd) {
            match invoke_handler(
                &registration,
                HookEvent::BeforeRunEnd(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::BeforeRunEnd(result)) => {
                    if let Some(value) = result.and_then(|v| v.follow_up) {
                        follow_up = Some(value);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::BeforeRunEnd,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::BeforeRunEnd(Some(BeforeRunEndResult {
            follow_up,
        })))
    }

    async fn transform_context(
        &self,
        mut event: TransformContextEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        for registration in self.registrations_for(HookName::TransformContext) {
            match invoke_handler(
                &registration,
                HookEvent::TransformContext(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::TransformContext(result)) => {
                    if let Some(result) = result {
                        if let Some(messages) = result.messages {
                            event.messages = messages;
                        }
                        if let Some(system_prompt) = result.system_prompt {
                            event.system_prompt = system_prompt;
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::TransformContext,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::TransformContext(Some(TransformContextResult {
            messages: Some(event.messages),
            system_prompt: Some(event.system_prompt),
        })))
    }

    async fn before_request(
        &self,
        mut event: BeforeRequestEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        for registration in self.registrations_for(HookName::BeforeRequest) {
            match invoke_handler(
                &registration,
                HookEvent::BeforeRequest(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::BeforeRequest(result)) => {
                    if let Some(stream_options) = result.and_then(|v| v.stream_options) {
                        event.stream_options = stream_options;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::BeforeRequest,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::BeforeRequest(Some(BeforeRequestResult {
            stream_options: Some(event.stream_options),
        })))
    }

    async fn before_payload(
        &self,
        mut event: BeforePayloadEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        for registration in self.registrations_for(HookName::BeforePayload) {
            match invoke_handler(
                &registration,
                HookEvent::BeforePayload(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::BeforePayload(result)) => {
                    if let Some(result) = result {
                        event.payload = result.payload;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::BeforePayload,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::BeforePayload(Some(BeforePayloadResult {
            payload: event.payload,
        })))
    }

    async fn after_response(
        &self,
        mut event: AfterResponseEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        for registration in self.registrations_for(HookName::AfterResponse) {
            match invoke_handler(
                &registration,
                HookEvent::AfterResponse(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::AfterResponse(result)) => {
                    if let Some(message) = result.and_then(|v| v.message) {
                        event.message = message;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::AfterResponse,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::AfterResponse(Some(AfterResponseResult {
            message: Some(event.message),
        })))
    }
    async fn before_tool(
        &self,
        mut event: BeforeToolEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        let mut block = None;
        for registration in self.registrations_for(HookName::BeforeTool) {
            match self
                .invoke_tool_registration(
                    HookName::BeforeTool,
                    &registration,
                    HookEvent::BeforeTool(event.clone()),
                    context.clone(),
                )
                .await
            {
                Ok(HookResult::BeforeTool(result)) => {
                    let result = result.unwrap_or_default();
                    if let Some(args) = result.args {
                        event.args = args;
                    }
                    if let Some(value) = result.block {
                        block = Some(value);
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error.clone(),
                        HookName::BeforeTool,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await;
                    block = Some(ToolBlock {
                        reason: error.message,
                        terminate: None,
                    });
                    break;
                }
            }
        }
        Ok(HookResult::BeforeTool(Some(BeforeToolResult {
            args: Some(event.args),
            block,
        })))
    }

    async fn after_tool(
        &self,
        mut event: AfterToolEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        let mut aggregate = AfterToolResult::default();
        let mut changed = false;
        for registration in self.registrations_for(HookName::AfterTool) {
            match self
                .invoke_tool_registration(
                    HookName::AfterTool,
                    &registration,
                    HookEvent::AfterTool(event.clone()),
                    context.clone(),
                )
                .await
            {
                Ok(HookResult::AfterTool(result)) => {
                    let Some(result) = result else { continue };
                    if let Some(value) = result.content {
                        aggregate.content = Some(value.clone());
                        event.content = value;
                        changed = true;
                    }
                    if let Some(value) = result.details {
                        aggregate.details = Some(value.clone());
                        event.details = Some(value);
                        changed = true;
                    }
                    if let Some(value) = result.is_error {
                        aggregate.is_error = Some(value);
                        event.is_error = value;
                        changed = true;
                    }
                    if let Some(value) = result.usage {
                        aggregate.usage = Some(value.clone());
                        event.usage = Some(value);
                        changed = true;
                    }
                    if let Some(value) = result.terminate {
                        aggregate.terminate = Some(value);
                        changed = true;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::AfterTool,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::AfterTool(changed.then_some(aggregate)))
    }

    async fn before_compaction(
        &self,
        event: BeforeCompactionEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        for registration in self.registrations_for(HookName::BeforeCompaction) {
            match invoke_handler(
                &registration,
                HookEvent::BeforeCompaction(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::BeforeCompaction(result)) => {
                    let result = result.unwrap_or_default();
                    if result.decline == Some(true) && result.compaction.is_some() {
                        let error = HookError::handler(
                            "before_compaction hook cannot return both decline and compaction",
                        );
                        self.report(
                            error,
                            HookName::BeforeCompaction,
                            event.lane.clone(),
                            context.clone(),
                        )
                        .await;
                        continue;
                    }
                    if result.decline == Some(true) || result.compaction.is_some() {
                        return Ok(HookResult::BeforeCompaction(Some(result)));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::BeforeCompaction,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::BeforeCompaction(None))
    }

    async fn before_navigation(
        &self,
        event: BeforeNavigationEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        for registration in self.registrations_for(HookName::BeforeNavigation) {
            match invoke_handler(
                &registration,
                HookEvent::BeforeNavigation(event.clone()),
                context.clone(),
            )
            .await
            {
                Ok(HookResult::BeforeNavigation(result)) => {
                    let result = result.unwrap_or_default();
                    if result.decline == Some(true) && result.summary.is_some() {
                        let error = HookError::handler(
                            "before_navigation hook cannot return both decline and summary",
                        );
                        self.report(
                            error,
                            HookName::BeforeNavigation,
                            event.lane.clone(),
                            context.clone(),
                        )
                        .await;
                        continue;
                    }
                    if result.decline == Some(true) || result.summary.is_some() {
                        return Ok(HookResult::BeforeNavigation(Some(result)));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.report(
                        error,
                        HookName::BeforeNavigation,
                        event.lane.clone(),
                        context.clone(),
                    )
                    .await
                }
            }
        }
        Ok(HookResult::BeforeNavigation(None))
    }
    async fn invoke_tool_registration(
        &self,
        name: HookName,
        registration: &HookRegistration,
        event: HookEvent,
        context: Context,
    ) -> Result<HookResult, HookError> {
        let telemetry = context.telemetry().cloned();
        let telemetry_ref: &dyn TelemetryContext = telemetry.as_deref().unwrap_or(&NOOP);
        let lane = event.lane().to_string();
        let run_id = event.run_id().to_string();
        let hook_name = name.as_str();
        let registration_id = registration.id.clone();
        let handler = registration.handler.clone();
        let handler_context = context.clone();
        start_harness_hook_span(
            telemetry_ref,
            &lane,
            &run_id,
            hook_name,
            registration_id.as_deref(),
            move |span| async move {
                let result = handler(event, handler_context).await;
                let outcome = match &result {
                    Ok(HookResult::BeforeTool(Some(result))) if result.block.is_some() => "blocked",
                    Ok(_) => "completed",
                    Err(error) => {
                        span.record_error(&error.message);
                        "failed"
                    }
                };
                span.set_attribute(
                    "pi.hook.outcome",
                    crate::agent::harness::telemetry::AttributeValue::from(outcome),
                );
                result
            },
        )
        .await
    }

    async fn report(&self, error: HookError, name: HookName, lane: String, context: Context) {
        (self.report_error.clone())(error, name, lane, context).await;
    }

    fn registrations_for(&self, name: HookName) -> Vec<HookRegistration> {
        self.lock()
            .registrations
            .get(&name)
            .cloned()
            .unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

async fn invoke_handler(
    registration: &HookRegistration,
    event: HookEvent,
    context: Context,
) -> Result<HookResult, HookError> {
    (registration.handler.clone())(event, context).await
}

macro_rules! hook_matcher {
    ($name:ident, $variant:ident, $type:ty) => {
        fn $name(event: HookEvent, _hook: HookName) -> Result<$type, HookError> {
            match event {
                HookEvent::$variant(value) => Ok(value),
                _ => Err(HookError::handler("hook event does not match hook name")),
            }
        }
    };
}

hook_matcher!(expect_before_run, BeforeRun, BeforeRunEvent);
hook_matcher!(expect_before_drive, BeforeDrive, BeforeDriveEvent);
hook_matcher!(expect_before_run_end, BeforeRunEnd, BeforeRunEndEvent);
hook_matcher!(
    expect_transform_context,
    TransformContext,
    TransformContextEvent
);
hook_matcher!(expect_before_request, BeforeRequest, BeforeRequestEvent);
hook_matcher!(expect_before_payload, BeforePayload, BeforePayloadEvent);
hook_matcher!(expect_after_response, AfterResponse, AfterResponseEvent);
hook_matcher!(expect_before_tool, BeforeTool, BeforeToolEvent);
hook_matcher!(expect_after_tool, AfterTool, AfterToolEvent);
hook_matcher!(
    expect_before_compaction,
    BeforeCompaction,
    BeforeCompactionEvent
);
hook_matcher!(
    expect_before_navigation,
    BeforeNavigation,
    BeforeNavigationEvent
);
#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::types::TextOrImageContent;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn context() -> Context {
        Context::new().with_signal(tokio_util::sync::CancellationToken::new())
    }

    fn user_message(text: &str) -> AgentMessage {
        AgentMessage::user_text(text.to_string(), 1)
    }

    fn before_run_event(prompt: Vec<AgentMessage>) -> HookEvent {
        HookEvent::BeforeRun(BeforeRunEvent {
            lane: "main".to_string(),
            run_id: "run-1".to_string(),
            prompt,
            resources: json!({}),
        })
    }

    #[tokio::test]
    async fn before_run_accumulates_injected_messages() {
        let registry = HookRegistry::default();
        registry
            .on(
                HookName::BeforeRun,
                Arc::new(|_, _| {
                    Box::pin(async move {
                        Ok(HookResult::BeforeRun(Some(BeforeRunResult {
                            messages: Some(vec![user_message("first")]),
                        })))
                    })
                }),
                Some("first".to_string()),
            )
            .unwrap();
        registry
            .on(
                HookName::BeforeRun,
                Arc::new(|event, _| {
                    Box::pin(async move {
                        let HookEvent::BeforeRun(event) = event else {
                            return Err(HookError::handler("event mismatch"));
                        };
                        assert_eq!(event.prompt.len(), 2);
                        Ok(HookResult::BeforeRun(Some(BeforeRunResult {
                            messages: Some(vec![user_message("second")]),
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let Some(HookResult::BeforeRun(Some(result))) = registry
            .run_with_gate(
                HookName::BeforeRun,
                before_run_event(vec![user_message("base")]),
                &gate,
                context(),
            )
            .await
            .unwrap()
        else {
            panic!("expected before_run result");
        };
        assert_eq!(result.messages.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn before_drive_is_fail_closed() {
        let registry = HookRegistry::default();
        registry
            .on(
                HookName::BeforeDrive,
                Arc::new(|_, _| Box::pin(async move { Err(HookError::handler("denied")) })),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let error = registry
            .run_with_gate(
                HookName::BeforeDrive,
                HookEvent::BeforeDrive(BeforeDriveEvent {
                    lane: "main".to_string(),
                    run_id: "run-1".to_string(),
                    operation: DriveOperation::Run,
                }),
                &gate,
                context(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.message, "denied");
    }

    #[tokio::test]
    async fn transform_context_threads_messages_and_system_prompt() {
        let registry = HookRegistry::default();
        registry
            .on(
                HookName::TransformContext,
                Arc::new(|event, _| {
                    Box::pin(async move {
                        let HookEvent::TransformContext(mut event) = event else {
                            return Err(HookError::handler("event mismatch"));
                        };
                        event.messages.push(user_message("injected"));
                        event.system_prompt.push_str(" + one");
                        Ok(HookResult::TransformContext(Some(TransformContextResult {
                            messages: Some(event.messages),
                            system_prompt: Some(event.system_prompt),
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let Some(HookResult::TransformContext(Some(result))) = registry
            .run_with_gate(
                HookName::TransformContext,
                HookEvent::TransformContext(TransformContextEvent {
                    lane: "main".to_string(),
                    run_id: "run-1".to_string(),
                    messages: vec![user_message("base")],
                    system_prompt: "base".to_string(),
                }),
                &gate,
                context(),
            )
            .await
            .unwrap()
        else {
            panic!("expected transform result");
        };
        assert_eq!(result.messages.unwrap().len(), 2);
        assert_eq!(result.system_prompt.unwrap(), "base + one");
    }

    #[tokio::test]
    async fn before_tool_threads_args_stops_at_block_and_records_span() {
        let telemetry = Arc::new(crate::agent::harness::telemetry::InMemoryTelemetryContext::new());
        let registry = HookRegistry::default();
        registry
            .on(
                HookName::BeforeTool,
                Arc::new(|event, _| {
                    Box::pin(async move {
                        let HookEvent::BeforeTool(mut event) = event else {
                            return Err(HookError::handler("event mismatch"));
                        };
                        event.args["patched"] = json!(true);
                        Ok(HookResult::BeforeTool(Some(BeforeToolResult {
                            args: Some(event.args),
                            block: Some(ToolBlock {
                                reason: "blocked".to_string(),
                                terminate: Some(true),
                            }),
                        })))
                    })
                }),
                Some("guard".to_string()),
            )
            .unwrap();
        let late = Arc::new(AtomicBool::new(false));
        let seen = late.clone();
        registry
            .on(
                HookName::BeforeTool,
                Arc::new(move |_, _| {
                    let seen = seen.clone();
                    Box::pin(async move {
                        seen.store(true, Ordering::SeqCst);
                        Ok(HookResult::Unit)
                    })
                }),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let tool_context = context().with_telemetry(telemetry.clone());
        let Some(HookResult::BeforeTool(Some(result))) = registry
            .run_tool_with_gate(
                HookName::BeforeTool,
                HookEvent::BeforeTool(BeforeToolEvent {
                    lane: "main".to_string(),
                    run_id: "run-1".to_string(),
                    tool_call_id: "call".to_string(),
                    tool_name: "read".to_string(),
                    args: json!({}),
                }),
                &gate,
                tool_context,
            )
            .await
            .unwrap()
        else {
            panic!("expected before_tool result");
        };
        assert_eq!(result.args.unwrap()["patched"], json!(true));
        assert_eq!(result.block.unwrap().reason, "blocked");
        assert!(!late.load(Ordering::SeqCst));
        let spans = telemetry.recorded_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "pi.harness.hook");
        assert_eq!(
            spans[0].attributes.get("pi.hook.outcome"),
            Some(&crate::agent::harness::telemetry::AttributeValue::from(
                "blocked"
            ))
        );
    }

    #[tokio::test]
    async fn after_tool_aggregates_field_patches_and_terminate() {
        let registry = HookRegistry::default();
        registry
            .on(
                HookName::AfterTool,
                Arc::new(|_, _| {
                    Box::pin(async move {
                        Ok(HookResult::AfterTool(Some(AfterToolResult {
                            content: Some(vec![TextOrImageContent::text("patched")]),
                            is_error: Some(true),
                            terminate: Some(true),
                            ..Default::default()
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let Some(HookResult::AfterTool(Some(result))) = registry
            .run_tool_with_gate(
                HookName::AfterTool,
                HookEvent::AfterTool(AfterToolEvent {
                    lane: "main".to_string(),
                    run_id: "run-1".to_string(),
                    tool_call_id: "call".to_string(),
                    tool_name: "read".to_string(),
                    args: json!({}),
                    content: vec![TextOrImageContent::text("original")],
                    details: None,
                    is_error: false,
                    usage: None,
                }),
                &gate,
                context(),
            )
            .await
            .unwrap()
        else {
            panic!("expected after_tool result");
        };
        assert_eq!(
            result.content.unwrap()[0],
            TextOrImageContent::text("patched")
        );
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.terminate, Some(true));
    }

    #[tokio::test]
    async fn before_compaction_first_structural_hook_wins() {
        let registry = HookRegistry::default();
        registry
            .on(
                HookName::BeforeCompaction,
                Arc::new(|_, _| {
                    Box::pin(async move {
                        Ok(HookResult::BeforeCompaction(Some(BeforeCompactionResult {
                            decline: Some(true),
                            compaction: None,
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        registry
            .on(
                HookName::BeforeCompaction,
                Arc::new(|_, _| {
                    Box::pin(async move {
                        Ok(HookResult::BeforeCompaction(Some(BeforeCompactionResult {
                            decline: Some(false),
                            compaction: Some(json!({"summary": "custom"})),
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let Some(HookResult::BeforeCompaction(Some(result))) = registry
            .run_with_gate(
                HookName::BeforeCompaction,
                HookEvent::BeforeCompaction(BeforeCompactionEvent {
                    lane: "main".to_string(),
                    run_id: "run-1".to_string(),
                    reason: HookCompactionReason::Manual,
                    preparation: json!({}),
                    custom_instructions: None,
                }),
                &gate,
                context(),
            )
            .await
            .unwrap()
        else {
            panic!("expected compaction result");
        };
        assert_eq!(result.decline, Some(true));
        assert!(result.compaction.is_none());
    }

    #[tokio::test]
    async fn before_payload_threads_payload_and_before_run_reports_handler_errors() {
        let errors = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reporter_errors = errors.clone();
        let registry = HookRegistry::with_reporter(Arc::new(move |error, name, _, _| {
            let errors = reporter_errors.clone();
            Box::pin(async move {
                errors
                    .lock()
                    .unwrap()
                    .push((name.as_str().to_string(), error.message));
            })
        }));
        registry
            .on(
                HookName::BeforeRun,
                Arc::new(|_, _| Box::pin(async move { Err(HookError::handler("boom")) })),
                None,
            )
            .unwrap();
        registry
            .on(
                HookName::BeforeRun,
                Arc::new(|_, _| {
                    Box::pin(async move {
                        Ok(HookResult::BeforeRun(Some(BeforeRunResult {
                            messages: Some(vec![user_message("continued")]),
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        registry
            .on(
                HookName::BeforePayload,
                Arc::new(|event, _| {
                    Box::pin(async move {
                        let HookEvent::BeforePayload(mut event) = event else {
                            return Err(HookError::handler("event mismatch"));
                        };
                        event.payload["patched"] = json!(1);
                        Ok(HookResult::BeforePayload(Some(BeforePayloadResult {
                            payload: event.payload,
                        })))
                    })
                }),
                None,
            )
            .unwrap();
        let (gate, _control) = crate::agent::harness::execution::create_gate();
        let run = registry
            .run_with_gate(
                HookName::BeforeRun,
                before_run_event(vec![]),
                &gate,
                context(),
            )
            .await
            .unwrap();
        assert!(matches!(run, Some(HookResult::BeforeRun(Some(_)))));
        assert_eq!(
            errors.lock().unwrap().clone(),
            vec![("before_run".to_string(), "boom".to_string())]
        );
        let Some(HookResult::BeforePayload(Some(result))) = registry
            .run_with_gate(
                HookName::BeforePayload,
                HookEvent::BeforePayload(BeforePayloadEvent {
                    lane: "main".to_string(),
                    run_id: "run-1".to_string(),
                    model: "openai/gpt".to_string(),
                    payload: json!({}),
                }),
                &gate,
                context(),
            )
            .await
            .unwrap()
        else {
            panic!("expected payload result");
        };
        assert_eq!(result.payload["patched"], json!(1));
    }

    #[test]
    fn registry_subscription_counts_and_close() {
        let registry = HookRegistry::default();
        let subscription = registry
            .on(
                HookName::AfterTool,
                Arc::new(|_, _| Box::pin(async { Ok(HookResult::Unit) })),
                None,
            )
            .unwrap();
        assert_eq!(registry.count(HookName::AfterTool), 1);
        assert!(registry.has(HookName::AfterTool));
        subscription.unsubscribe();
        assert!(!registry.has(HookName::AfterTool));
        registry.close("stopped");
        assert_eq!(registry.closed_error(), Some("stopped".to_string()));
        assert!(registry
            .on(
                HookName::AfterTool,
                Arc::new(|_, _| Box::pin(async { Ok(HookResult::Unit) })),
                None
            )
            .is_err());
        assert_eq!(registry.counts()[8], 0);
    }
}
