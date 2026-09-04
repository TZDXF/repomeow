//! pi-ai 核心类型契约:对齐 `packages/ai/src/types.ts`(0.84.4)。
//!
//! 序列化格式与 TS 版 JSON 完全兼容(camelCase 字段、tag 值一致),以便
//! JSONL 会话存储、跨端调试与前端消费共享同一形状。
//! 有意精简:provider 请求回调(onPayload/onResponse)以 Rust 回调字段存在、
//! 不参与序列化;`deferred` 相关(Unused in openai-completions)暂不建模。

use std::collections::HashMap;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::event_stream::EventStream;

/// 已知 API 名。TS 为 `KnownApi | (string & {})`,Rust 用 String 承载。
pub const API_OPENAI_COMPLETIONS: &str = "openai-completions";
pub const API_OPENAI_RESPONSES: &str = "openai-responses";
pub const API_ANTHROPIC_MESSAGES: &str = "anthropic-messages";
pub const API_GOOGLE_GENERATIVE_AI: &str = "google-generative-ai";

pub const SUPPORTED_APIS: [&str; 4] = [
    API_OPENAI_COMPLETIONS,
    API_OPENAI_RESPONSES,
    API_ANTHROPIC_MESSAGES,
    API_GOOGLE_GENERATIVE_AI,
];

pub fn is_supported_api(api: &str) -> bool {
    SUPPORTED_APIS.contains(&api)
}

pub type Api = String;
pub type ProviderId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    Auto,
    None,
}

/// pi-ai 的 `ThinkingLevel`(不含 off;agent 层使用 [`ModelThinkingLevel`])。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// pi-ai 的 `ModelThinkingLevel` = `"off" | ThinkingLevel`;
/// agent-core 的 `ThinkingLevel` 即此类型(在 `agent::types` 重导出)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Token budgets for each thinking level (token-based providers only)。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Sse,
    Websocket,
    WebsocketCached,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// provider 请求发出前观测/改写 payload 的回调(Rust 侧不序列化)。
pub type OnPayloadFn = Box<dyn Fn(Value) -> BoxFuture<'static, Option<Value>> + Send + Sync>;
/// 收到 HTTP 响应后的回调(Rust 侧不序列化)。
pub type OnResponseFn = Box<dyn Fn(&ProviderResponse) + Send + Sync>;

/// pi-ai `StreamOptions` 与 `SimpleStreamOptions` 合并为单一结构
/// (TS 里后者 extends 前者,拆分在 Rust 无收益)。
/// `signal` 不在结构内:Rust 侧以 `AbortSignal` 参数显式传递。
#[derive(Default)]
pub struct SimpleStreamOptions {
    pub api_key: Option<String>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    /// Arbitrary sampling parameters merged into the request body as-is。
    pub sampling_params: Option<HashMap<String, Value>>,
    /// Custom HTTP headers;值覆盖 provider 默认。
    pub headers: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    /// Provider-scoped environment overrides(优先于进程环境)。
    pub env: Option<HashMap<String, String>>,
    /// 可选 fetch 实现;None 时由 provider 使用默认 reqwest 客户端。
    pub transport: Option<Transport>,
    pub cache_retention: Option<CacheRetention>,
    /// Providers 可用于 prompt 缓存/路由的会话标识。
    pub session_id: Option<String>,
    /// Providers 提取其理解字段的元数据。
    pub metadata: Option<HashMap<String, Value>>,
    /// Provider-neutral tool selection;缺省时 adapter 用自身默认行为。
    pub tool_choice: Option<ToolChoice>,
    /// Provider-neutral reasoning level(`completeSimple`/`streamSimple` 语义)。
    pub reasoning: Option<ThinkingLevel>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub on_payload: Option<OnPayloadFn>,
    pub on_response: Option<OnResponseFn>,
}

impl Clone for SimpleStreamOptions {
    fn clone(&self) -> Self {
        // 回调闭包不可克隆;克隆副本不携带观测回调(与 TS spread 语义差异点,已确认无害)。
        Self {
            api_key: self.api_key.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            sampling_params: self.sampling_params.clone(),
            headers: self.headers.clone(),
            timeout_ms: self.timeout_ms,
            max_retries: self.max_retries,
            max_retry_delay_ms: self.max_retry_delay_ms,
            env: self.env.clone(),
            transport: self.transport,
            cache_retention: self.cache_retention,
            session_id: self.session_id.clone(),
            metadata: self.metadata.clone(),
            tool_choice: self.tool_choice,
            reasoning: self.reasoning,
            thinking_budgets: self.thinking_budgets.clone(),
            on_payload: None,
            on_response: None,
        }
    }
}

impl std::fmt::Debug for SimpleStreamOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimpleStreamOptions")
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("sampling_params", &self.sampling_params)
            .field("headers", &self.headers)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_retries", &self.max_retries)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .field("env", &self.env)
            .field("transport", &self.transport)
            .field("cache_retention", &self.cache_retention)
            .field("session_id", &self.session_id)
            .field("metadata", &self.metadata)
            .field("tool_choice", &self.tool_choice)
            .field("reasoning", &self.reasoning)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("has_on_payload", &self.on_payload.is_some())
            .field("has_on_response", &self.on_response.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    /// Provider-specific metadata(如 OpenAI Responses 的 legacy id / TextSignatureV1)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            text_signature: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    /// Provider-specific opaque reasoning replay data。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    /// 安全过滤导致思考被抹除时,密文载荷存于 thinking_signature。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub redacted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    /// base64 encoded image data。
    pub data: String,
    /// 例如 "image/jpeg"、"image/png"。
    pub mime_type: String,
}

/// 一次工具调用内容块(assistant 输出)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Map<String, Value>,
    /// Google 专用:复用 thought 上下文的签名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// assistant 消息内容块(对齐 TS `(TextContent | ThinkingContent | ToolCall)[]`)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistantContent {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking_signature: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
    },
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

impl AssistantContent {
    pub fn text(text: impl Into<String>) -> Self {
        AssistantContent::Text {
            text: text.into(),
            text_signature: None,
        }
    }
}

/// toolResult / user 支持的内容块(Text|Image)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TextOrImageContent {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_signature: Option<String>,
    },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
}

impl TextOrImageContent {
    pub fn text(text: impl Into<String>) -> Self {
        TextOrImageContent::Text {
            text: text.into(),
            text_signature: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub total: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input: i64,
    #[serde(default)]
    pub output: i64,
    #[serde(default)]
    pub cache_read: i64,
    #[serde(default)]
    pub cache_write: i64,
    /// 仅 Anthropic 上报的 1h 保留写入子集。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<i64>,
    /// reasoning tokens;为 output 的子集,provider 不上报时缺省。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<i64>,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub cost: UsageCost,
}

impl Usage {
    pub fn zero() -> Self {
        Self::default()
    }

    /// 逐项累加(用于多回合聚合)。
    pub fn add(&mut self, other: &Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cache_write_1h = match (self.cache_write_1h, other.cache_write_1h) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        self.reasoning = match (self.reasoning, other.reasoning) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        self.total_tokens += other.total_tokens;
        self.cost.input += other.cost.input;
        self.cost.output += other.cost.output;
        self.cost.cache_read += other.cost.cache_read;
        self.cost.cache_write += other.cost.cache_write;
        self.cost.total += other.cost.total;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "length")]
    Length,
    #[serde(rename = "toolUse")]
    ToolUse,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "aborted")]
    Aborted,
    #[serde(rename = "deferred")]
    Deferred,
}

/// TS `UserMessage.content: string | (TextContent | ImageContent)[]`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<TextOrImageContent>),
}

impl UserContent {
    pub fn text(text: impl Into<String>) -> Self {
        UserContent::Text(text.into())
    }

    pub fn to_plain_text(&self) -> String {
        match self {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    TextOrImageContent::Text { text, .. } => Some(text.as_str()),
                    TextOrImageContent::Image { .. } => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub role: String, // 恒为 "user"(tag 承载)
    pub content: UserContent,
    /// Unix 时间戳(毫秒)。
    pub timestamp: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub role: String, // 恒为 "assistant"
    pub content: Vec<AssistantContent>,
    pub api: Api,
    pub provider: ProviderId,
    pub model: String,
    /// chunk.model 与请求 model 不同时记录(如 OpenRouter auto)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_stop_reason: Option<String>,
    /// provider 表明的显式结束标记;当前不影响 agent 控制流。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_turn: Option<bool>,
    /// Unix 时间戳(毫秒)。
    pub timestamp: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub role: String, // 恒为 "toolResult"
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: Vec<TextOrImageContent>,
    /// 结构化 details(UI/日志用),任意 JSON。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// 工具自身执行用量(不参与主 LLM 上下文记账)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// 本结果引入的工具名(延迟加载语义;openai-completions 忽略)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    pub is_error: bool,
    pub timestamp: i64,
}

/// 统一 Message 类型(与 TS `Message` union JSON 形状一致)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "camelCase")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

/// 工具定义;`parameters` 为 JSON Schema(TS 为 TypeBox schema,序列化形状一致)。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// LLM 请求上下文。
#[derive(Clone, Debug, Default)]
pub struct Context {
    pub system_prompt: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostRates {
    /// $/million tokens。
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
    #[serde(flatten)]
    pub rates: ModelCostRates,
    /// 输入总量超过该 token 数时启用此档。
    pub input_tokens_above: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    #[serde(flatten)]
    pub rates: ModelCostRates,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelCostTier>>,
}

/// OpenAI 兼容 completions 的兼容性开关(对齐 `OpenAICompletionsCompat` 的
/// openai-completions provider 实现所需子集;缺省值由 baseUrl 自动探测)。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompletionsCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_store: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_usage_in_streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_finish_reason: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<MaxTokensField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_tool_result_name: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_assistant_after_tool_result: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_thinking_as_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<ThinkingFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_long_cache_retention: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_max_output_tokens: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_eager_tool_input_streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_session_affinity_headers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_cache_control_on_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_temperature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_adaptive_thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_empty_signature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_tools: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaxTokensField {
    #[serde(rename = "max_completion_tokens")]
    MaxCompletionTokens,
    #[serde(rename = "max_tokens")]
    MaxTokens,
}

/// OpenAI 兼容端点的 thinking/reasoning 参数格式(对齐 TS thinkingFormat)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingFormat {
    Openai,
    Openrouter,
    Deepseek,
    Together,
    Baseten,
    Zai,
    Qwen,
    #[serde(rename = "chat-template")]
    ChatTemplate,
    #[serde(rename = "qwen-chat-template")]
    QwenChatTemplate,
    #[serde(rename = "string-thinking")]
    StringThinking,
    #[serde(rename = "ant-ling")]
    AntLing,
}

/// 顶层用于限制 reasoning tokens 的请求字段(vLLM/Qwen/DashScope/llama.cpp)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingTokenBudgetField {
    ThinkingTokenBudget,
    ThinkingBudget,
    ThinkingBudgetTokens,
}

/// 统一模型定义(对齐 `Model<Api>`)。本应用从 settings 三键构造单一模型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: ProviderId,
    pub base_url: String,
    pub reasoning: bool,
    /// pi thinking level → provider/model 特定值的映射;缺键用 provider 默认。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<HashMap<String, Option<String>>>,
    pub input: Vec<InputKind>,
    pub cost: ModelCost,
    pub context_window: i64,
    pub max_tokens: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_params: Option<HashMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// OpenAI 兼容 API 的兼容覆盖;缺省由 baseUrl 自动探测。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<OpenAICompletionsCompat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Text,
    Image,
}

impl Model {
    /// 从应用 AI 配置构造最小模型(api 固定 openai-completions,成本未知置 0)。
    /// 现仅测试用;正式路径经 `ai::catalog::resolve_model` 填全元数据。
    #[cfg(test)]
    pub fn from_settings(id: impl Into<String>, base_url: impl Into<String>) -> Self {
        let id: String = id.into();
        Self {
            name: id.clone(),
            id,
            api: API_OPENAI_COMPLETIONS.to_string(),
            provider: "custom".to_string(),
            base_url: base_url.into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![InputKind::Text],
            cost: ModelCost {
                rates: ModelCostRates {
                    input: 0.0,
                    output: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                tiers: None,
            },
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            headers: None,
            compat: None,
        }
    }
}

/// AssistantMessageEventStream 的流协议(12 个事件,tag 值与 TS 一致)。
/// 流必须先发 `start`,终止于 `done`(成功)或 `error`(失败/中止);
/// 失败以 stopReason "error"/"aborted" 的最终 AssistantMessage 编码,不抛异常。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum AssistantMessageEvent {
    #[serde(rename = "start")]
    Start { partial: AssistantMessage },
    #[serde(rename = "text_start")]
    TextStart {
        content_index: u32,
        partial: AssistantMessage,
    },
    #[serde(rename = "text_delta")]
    TextDelta {
        content_index: u32,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "text_end")]
    TextEnd {
        content_index: u32,
        content: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_start")]
    ThinkingStart {
        content_index: u32,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        content_index: u32,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "thinking_end")]
    ThinkingEnd {
        content_index: u32,
        content: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_start")]
    ToolcallStart {
        content_index: u32,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_delta")]
    ToolcallDelta {
        content_index: u32,
        delta: String,
        partial: AssistantMessage,
    },
    #[serde(rename = "toolcall_end")]
    ToolcallEnd {
        content_index: u32,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    #[serde(rename = "done")]
    Done {
        /// TS Extract<StopReason, "stop" | "length" | "toolUse" | "deferred">。
        reason: StopReason,
        message: AssistantMessage,
    },
    #[serde(rename = "error")]
    Error {
        /// TS Extract<StopReason, "aborted" | "error">。
        reason: StopReason,
        error: AssistantMessage,
    },
}

/// agent-loop 消费的流类型。
pub type AssistantMessageEventStream = EventStream<AssistantMessageEvent, AssistantMessage>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de>>(value: &T) -> T {
        serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
    }

    #[test]
    fn assistant_message_json_matches_ts_shape() {
        let message = AssistantMessage {
            role: "assistant".into(),
            content: vec![
                AssistantContent::Thinking {
                    thinking: "hmm".into(),
                    thinking_signature: None,
                    redacted: false,
                },
                AssistantContent::text("answer"),
                AssistantContent::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: serde_json::from_value(json!({"city": "Oslo"})).unwrap(),
                    thought_signature: None,
                    namespace: None,
                }),
            ],
            api: "openai-completions".into(),
            provider: "custom".into(),
            model: "gpt-test".into(),
            response_model: None,
            response_id: None,
            usage: Usage::zero(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 1_700_000_000_000,
        };

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"][1]["type"], "text");
        assert_eq!(value["content"][2]["type"], "toolCall");
        assert_eq!(value["content"][2]["id"], "call_1");
        assert_eq!(value["stopReason"], "toolUse");

        let back: AssistantMessage = roundtrip(&message);
        assert_eq!(back, message);
    }

    #[test]
    fn tool_result_message_json_matches_ts_shape() {
        let result = ToolResultMessage {
            role: "toolResult".into(),
            tool_call_id: "call_1".into(),
            tool_name: "get_weather".into(),
            content: vec![TextOrImageContent::text("18C")],
            details: Some(json!({"temp": 18})),
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 1_700_000_000_001,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["role"], "toolResult");
        assert_eq!(value["toolCallId"], "call_1");
        assert_eq!(value["isError"], false);
        let back: ToolResultMessage = roundtrip(&result);
        assert_eq!(back, result);
    }

    #[test]
    fn stream_events_keep_ts_tag_values() {
        let assistant = AssistantMessage {
            role: "assistant".into(),
            content: vec![],
            api: "openai-completions".into(),
            provider: "custom".into(),
            model: "m".into(),
            response_model: None,
            response_id: None,
            usage: Usage::zero(),
            stop_reason: StopReason::Stop,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        };
        let delta = AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "he".into(),
            partial: assistant.clone(),
        };
        let value = serde_json::to_value(&delta).unwrap();
        assert_eq!(value["type"], "text_delta");
        assert_eq!(value["delta"], "he");

        let done = AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: assistant,
        };
        let value = serde_json::to_value(&done).unwrap();
        assert_eq!(value["type"], "done");
        assert_eq!(value["reason"], "stop");
    }

    #[test]
    fn usage_add_accumulates_all_fields() {
        let mut total = Usage {
            input: 10,
            output: 5,
            cache_read: 3,
            cache_write: 1,
            reasoning: Some(2),
            total_tokens: 15,
            cost: UsageCost {
                input: 0.1,
                output: 0.2,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.3,
            },
            ..Default::default()
        };
        total.add(&Usage {
            input: 1,
            output: 2,
            cache_read: 0,
            cache_write: 0,
            reasoning: Some(1),
            total_tokens: 3,
            cost: UsageCost {
                input: 0.01,
                output: 0.02,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.03,
            },
            ..Default::default()
        });
        assert_eq!(total.input, 11);
        assert_eq!(total.output, 7);
        assert_eq!(total.total_tokens, 18);
        assert_eq!(total.reasoning, Some(3));
        assert!((total.cost.total - 0.33).abs() < 1e-9);
    }
}
