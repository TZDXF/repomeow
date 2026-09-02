//! pi-agent-core 核心类型契约:对齐 `packages/agent/src/types.ts`(0.84.4)。
//!
//! 序列化格式与 TS 版 JSON 兼容;`AgentMessage` 通过 untagged 双层建模支持
//! TS 的 declaration-merging 自定义消息(未知 role 落入 `Custom` 的 JSON map)。

use std::collections::HashSet;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub use crate::agent::llm::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Context, Message, Model,
    ModelThinkingLevel, TextOrImageContent, Tool, ToolCall, ToolResultMessage, Usage, UserContent,
    UserMessage,
};

/// TS `AbortSignal` 对应物:项目统一使用 `CancellationToken`。
pub type AbortSignal = CancellationToken;

/// agent 层 thinking 级别(含 off)。
pub type ThinkingLevelWithOff = ModelThinkingLevel;

/// agent-loop 使用的流函数。`Models.streamSimple` 满足此形状。
///
/// 契约:
/// - 绝不 panic / 返回 rejected future(请求/模型/运行时失败都必须编码进流);
/// - 返回 `AssistantMessageEventStream`;
/// - 失败经协议事件与 stopReason "error"/"aborted" 的最终 AssistantMessage 表达。
pub type StreamFn = Arc<
    dyn Fn(
            Model,
            Context,
            Option<SimpleStreamOptions>,
        ) -> BoxFuture<'static, AssistantMessageEventStream>
        + Send
        + Sync,
>;

use crate::agent::llm::SimpleStreamOptions;

/// 单条 assistant 消息内工具调用的执行方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

/// 排队用户消息在 drain 点的注入数量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueueMode {
    #[default]
    #[serde(rename = "one-at-a-time")]
    OneAtATime,
    All,
}

/// 工具执行的流式部分结果回调;仅在 execute 未完成时有效。
pub type AgentToolUpdateCallback = Arc<dyn Fn(AgentToolResult) + Send + Sync>;

/// 工具执行错误:对齐 TS `throw`,错误消息会成为 error 工具结果的文本。
pub type ToolExecutionError = Box<dyn std::error::Error + Send + Sync>;

/// 工具参数准备钩子(校验前的兼容 shim)。
pub type PrepareArgumentsFn = Arc<dyn Fn(Value) -> Value + Send + Sync>;

/// 工具执行函数。
pub type ToolExecuteFn = Arc<
    dyn Fn(
            String,
            Value,
            Option<AbortSignal>,
            Option<AgentToolUpdateCallback>,
        ) -> BoxFuture<'static, Result<AgentToolResult, ToolExecutionError>>
        + Send
        + Sync,
>;

/// agent 运行时的工具定义(对齐 TS `AgentTool` 对象)。
#[derive(Clone)]
pub struct AgentTool {
    /// LLM 调用标识。
    pub name: String,
    /// UI 展示标签。
    pub label: String,
    /// 告诉 LLM 何时以及如何使用。
    pub description: String,
    /// 参数 JSON Schema(对齐 TypeBox schema 的序列化形状)。
    pub parameters: Value,
    /// 本工具的执行方式覆盖;缺省用 loop 默认值。
    pub execution_mode: Option<ToolExecutionMode>,
    /// 校验前对原始参数的兼容转换。
    pub prepare_arguments: Option<PrepareArgumentsFn>,
    /// 执行工具;失败时返回 Err(转为 error 工具结果)。
    pub execute: ToolExecuteFn,
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("execution_mode", &self.execution_mode)
            .finish_non_exhaustive()
    }
}

impl AgentTool {
    /// 供 `Context.tools`(LLM 层)使用的视图。
    pub fn as_llm_tool(&self) -> Tool {
        Tool {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }
}

/// 工具的最终或部分结果。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    /// 回传给模型的文本/图片内容。
    pub content: Vec<TextOrImageContent>,
    /// 供日志或 UI 的结构化 details。
    #[serde(default)]
    pub details: Value,
    /// 工具自身执行的用量(如可用)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// 本结果引入的新工具名(延迟加载语义)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    /// 本批次全部结果都置位时,agent 在本批后提前终止。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub terminate: bool,
}

impl AgentToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![TextOrImageContent::text(text)],
            details: Value::Null,
            ..Default::default()
        }
    }
}

/// TS `AgentMessage = Message | CustomAgentMessages[...]` 的 Rust 建模:
/// 已知 role 强类型,其余(declaration merging 的自定义消息,如 harness 的
/// bashExecution/branchSummary)落入 `Custom` 原始 JSON map,由 harness 层
/// 提供类型化视图与 `convert_to_llm` 转换。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentMessage {
    Message(TypedMessage),
    Custom(serde_json::Map<String, Value>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum TypedMessage {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

impl AgentMessage {
    pub fn user_text(text: impl Into<String>, timestamp: i64) -> Self {
        AgentMessage::Message(TypedMessage::User(UserMessage {
            role: "user".to_string(),
            content: UserContent::text(text),
            timestamp,
        }))
    }

    pub fn role_name(&self) -> &str {
        match self {
            AgentMessage::Message(TypedMessage::User(_)) => "user",
            AgentMessage::Message(TypedMessage::Assistant(_)) => "assistant",
            AgentMessage::Message(TypedMessage::ToolResult(_)) => "toolResult",
            AgentMessage::Custom(map) => {
                map.get("role").and_then(Value::as_str).unwrap_or("custom")
            }
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            AgentMessage::Message(TypedMessage::User(m)) => m.timestamp,
            AgentMessage::Message(TypedMessage::Assistant(m)) => m.timestamp,
            AgentMessage::Message(TypedMessage::ToolResult(m)) => m.timestamp,
            AgentMessage::Custom(map) => map
                .get("timestamp")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        }
    }
}

/// 传入低层 agent loop 的上下文快照。
#[derive(Clone, Default)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<AgentTool>,
}

/// agent 公开状态(TS 的只读访问器在 Rust 侧由 `Agent` 的锁保护)。
#[derive(Clone)]
pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevelWithOff,
    pub tools: Vec<AgentTool>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: HashSet<String>,
    pub error_message: Option<String>,
}

/// `beforeToolCall` 返回:block 阻止执行并生成 error 工具结果;
/// terminate 参与"整批全部 terminate 才提前终止"的规则。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
    pub terminate: bool,
}

/// `afterToolCall` 返回的部分覆盖:逐字段替换,缺省保留原值,无深合并。
#[derive(Clone, Debug, Default)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<TextOrImageContent>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Usage>,
    pub terminate: Option<bool>,
}

/// `beforeToolCall` 收到的上下文。
pub struct BeforeToolCallContext {
    pub assistant_message: AssistantMessage,
    pub tool_call: ToolCall,
    pub args: Value,
    pub context: AgentContext,
}

/// `afterToolCall` 收到的上下文。
pub struct AfterToolCallContext {
    pub assistant_message: AssistantMessage,
    pub tool_call: ToolCall,
    pub args: Value,
    pub result: AgentToolResult,
    pub is_error: bool,
    pub context: AgentContext,
}

/// `shouldStopAfterTurn` / `prepareNextTurn` 收到的上下文。
pub struct ShouldStopAfterTurnContext {
    pub message: AssistantMessage,
    pub tool_results: Vec<ToolResultMessage>,
    pub context: AgentContext,
    /// 本次 loop 调用新增的消息(prompt 运行含初始 prompts;continuation 不含已有上下文)。
    pub new_messages: Vec<AgentMessage>,
}

pub type PrepareNextTurnContext = ShouldStopAfterTurnContext;

/// `prepareNextTurn` 返回的下一回合运行时状态替换。
#[derive(Default)]
pub struct AgentLoopTurnUpdate {
    pub context: Option<AgentContext>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevelWithOff>,
}

// ---------------------------------------------------------------------------
// Hook / StreamFn 类型别名
// ---------------------------------------------------------------------------

pub type AgentEventSink = Arc<dyn Fn(AgentEvent) -> BoxFuture<'static, ()> + Send + Sync>;

pub type ConvertToLlmFn =
    Arc<dyn Fn(Vec<AgentMessage>) -> BoxFuture<'static, Vec<Message>> + Send + Sync>;

pub type TransformContextFn = Arc<
    dyn Fn(Vec<AgentMessage>, Option<AbortSignal>) -> BoxFuture<'static, Vec<AgentMessage>>
        + Send
        + Sync,
>;

pub type GetApiKeyFn = Arc<dyn Fn(String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

pub type ShouldStopAfterTurnFn =
    Arc<dyn Fn(ShouldStopAfterTurnContext) -> BoxFuture<'static, bool> + Send + Sync>;

pub type PrepareNextTurnFn = Arc<
    dyn Fn(PrepareNextTurnContext) -> BoxFuture<'static, Option<AgentLoopTurnUpdate>> + Send + Sync,
>;

pub type GetQueuedMessagesFn = Arc<dyn Fn() -> BoxFuture<'static, Vec<AgentMessage>> + Send + Sync>;

pub type BeforeToolCallHookFn = Arc<
    dyn Fn(
            BeforeToolCallContext,
            Option<AbortSignal>,
        ) -> BoxFuture<'static, Option<BeforeToolCallResult>>
        + Send
        + Sync,
>;

pub type AfterToolCallHookFn = Arc<
    dyn Fn(
            AfterToolCallContext,
            Option<AbortSignal>,
        ) -> BoxFuture<'static, Option<AfterToolCallResult>>
        + Send
        + Sync,
>;

/// 低层 agent loop 配置(对齐 TS `AgentLoopConfig extends SimpleStreamOptions`)。
#[derive(Clone)]
pub struct AgentLoopConfig {
    pub model: Model,
    /// 每次请求附带的基础流选项(apiKey/reasoning/sessionId/onPayload/...)。
    pub stream: SimpleStreamOptions,
    /// AgentMessage[] → LLM Message[](必须不抛;不可转换的过滤掉)。
    pub convert_to_llm: ConvertToLlmFn,
    /// convertToLlm 之前的 AgentMessage 级上下文变换(窗口修剪/外部注入)。
    pub transform_context: Option<TransformContextFn>,
    /// 每次调用动态解析 API key(短时 OAuth token 场景)。
    pub get_api_key: Option<GetApiKeyFn>,
    /// turn_end 后询问是否优雅停止(不发起新 LLM 调用)。
    pub should_stop_after_turn: Option<ShouldStopAfterTurnFn>,
    /// 将继续下一回合时、turn 开始前的状态替换(如 compaction)。
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    /// 回合工具执行完后注入 steering 消息。
    pub get_steering_messages: Option<GetQueuedMessagesFn>,
    /// agent 即将停止时注入 follow-up 消息。
    pub get_follow_up_messages: Option<GetQueuedMessagesFn>,
    /// 多工具调用执行方式,默认 parallel。
    pub tool_execution: ToolExecutionMode,
    /// 工具执行前钩子(参数已校验)。
    pub before_tool_call: Option<BeforeToolCallHookFn>,
    /// 工具执行后、事件发出前的结果覆盖钩子。
    pub after_tool_call: Option<AfterToolCallHookFn>,
}

/// Agent 生命周期事件(对齐 TS `AgentEvent`,tag 与字段名 camelCase 兼容前端)。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    #[serde(rename = "agent_start")]
    AgentStart,
    #[serde(rename = "agent_end")]
    AgentEnd { messages: Vec<AgentMessage> },
    #[serde(rename = "turn_start")]
    TurnStart,
    #[serde(rename = "turn_end")]
    TurnEnd {
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
    },
    #[serde(rename = "message_start")]
    MessageStart { message: AgentMessage },
    /// 仅 assistant 消息流式期间发出。
    #[serde(rename = "message_update")]
    MessageUpdate {
        message: AgentMessage,
        assistant_message_event: AssistantMessageEvent,
    },
    #[serde(rename = "message_end")]
    MessageEnd { message: AgentMessage },
    #[serde(rename = "tool_execution_start")]
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    #[serde(rename = "tool_execution_update")]
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        partial_result: AgentToolResult,
    },
    #[serde(rename = "tool_execution_end")]
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: AgentToolResult,
        is_error: bool,
    },
}

/// Agent 事件监听器(按订阅顺序 await;收当前 run 的 abort signal)。
pub type AgentListener =
    Arc<dyn Fn(AgentEvent, AbortSignal) -> BoxFuture<'static, ()> + Send + Sync>;
