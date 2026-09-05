//! Anthropic Messages provider:pi-ai `api/anthropic-messages.ts`(0.84.4)的 Rust 复刻。
//!
//! 组成:
//! - [`build_request_body`]:纯函数,把 [`Model`] + [`Context`] + [`SimpleStreamOptions`]
//!   序列化为 Anthropic Messages 请求体(messages/system/tools/thinking/cache_control)。
//! - [`stream_anthropic_messages`]:流式入口,reqwest POST `{baseUrl}/v1/messages`,
//!   手工解码 SSE(message/content_block 事件),聚合为 [`AssistantMessageEvent`]
//!   推入 [`EventStreamWriter`];失败/中止编码进流(stopReason error/aborted +
//!   errorMessage),不 panic、不抛出。
//! - [`SseDecoder`] + [`AnthropicAggregator`]:SSE 文本解码与事件聚合均为纯逻辑,便于单测。
//!
//! 与蓝本的已知偏差:
//! - `Model.compat` 仍是 `OpenAICompletionsCompat`,Anthropic 专属 compat 用
//!   [`get_anthropic_compat`] 的文件内默认值(forceAdaptiveThinking/supportsMidConvoEffort
//!   恒 false,adaptive thinking/effort/fallbacks 路径未实现,待类型整合后接入)。
//! - 无 `tool.constrainedSampling` 建模 → strict tools 恒不启用。
//! - `providerThinkingLevel`/`insertThinkingLevelMessages`/`input_transformations`
//!   诊断/Rust 类型未建模,不实现;fallback 模型成本重映射不实现。
//! - provider 内层重试对齐 TS `retryProviderRequest`(x-should-retry 头优先,
//!   408/409/429/5xx、retry-after、指数退避)。
//! - `sanitizeSurrogates` 在 Rust 无意义(String 恒为合法 UTF-8),省略。
//!
//! 认证:`x-api-key`(普通)或 `Authorization: Bearer`(`sk-ant-oat` OAuth token /
//! github-copilot provider);OAuth 时注入 Claude Code 身份头与隐身工具命名。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::event_stream::{event_stream, EventStreamWriter};
use super::types::{
    user_agent, AssistantContent, AssistantMessage, AssistantMessageEvent,
    AssistantMessageEventStream, CacheRetention, Context, InputKind, Message, Model,
    ProviderResponse, SimpleStreamOptions, StopReason, TextOrImageContent, ThinkingBudgets,
    ThinkingLevel, Tool, ToolCall, ToolChoice, ToolResultMessage, Usage, UserContent,
};
use crate::time_util::now_ts_nanos;

// ── 常量 ─────────────────────────────────────────────────────────────

/// Stealth mode:假扮 Claude Code CLI 的版本号(对齐蓝本 claudeCodeVersion)。
const CLAUDE_CODE_VERSION: &str = "2.1.251";

/// Claude Code 2.x 规范工具名(OAuth 隐身模式下大小写归一目标)。
const CLAUDE_CODE_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

const CLAUDE_CODE_IDENTITY_PROMPT: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";
/// 纯图片内容块补占位文本(Anthropic 拒绝空 text 的纯图片消息)。
const IMAGE_ONLY_TEXT: &str = "(see attached image)";
const SYNTHETIC_TOOL_RESULT_TEXT: &str = "No result provided";

const MIN_ANSWER_TOKENS: i64 = 1024;
const DEFAULT_THINKING_BUDGET_MINIMAL: i64 = 1024;
const DEFAULT_THINKING_BUDGET_LOW: i64 = 2048;
const DEFAULT_THINKING_BUDGET_MEDIUM: i64 = 8192;
const DEFAULT_THINKING_BUDGET_HIGH: i64 = 16384;

/// SSE 层可处理的 Anthropic 消息事件集合(其余 event 名跳过)。
const ANTHROPIC_MESSAGE_EVENTS: [&str; 6] = [
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

// ── Claude Code 隐身命名 ──────────────────────────────────────────────

fn to_claude_code_name(name: &str) -> String {
    CLAUDE_CODE_TOOLS
        .iter()
        .find(|tool| tool.eq_ignore_ascii_case(name))
        .map(|tool| (*tool).to_string())
        .unwrap_or_else(|| name.to_string())
}

fn from_claude_code_name(name: &str, tools: &[Tool]) -> String {
    if !tools.is_empty() {
        if let Some(tool) = tools
            .iter()
            .find(|tool| tool.name.eq_ignore_ascii_case(name))
        {
            return tool.name.clone();
        }
    }
    name.to_string()
}

/// OAuth token(contains "sk-ant-oat")→ Bearer 认证 + Claude Code 身份。
fn is_oauth_token(api_key: &str) -> bool {
    api_key.contains("sk-ant-oat")
}

// ── compat ───────────────────────────────────────────────────────────

/// Anthropic Messages 解析后的兼容开关。
#[derive(Clone, Debug, PartialEq)]
pub struct AnthropicMessagesCompat {
    pub supports_eager_tool_input_streaming: bool,
    pub supports_long_cache_retention: bool,
    pub send_session_affinity_headers: bool,
    pub supports_cache_control_on_tools: bool,
    pub supports_temperature: bool,
    pub force_adaptive_thinking: bool,
    pub allow_empty_signature: bool,
    pub supports_strict_tools: bool,
    pub supports_mid_convo_effort: bool,
    pub supports_tool_references: bool,
}

/// TS getAnthropicCompat:显式模型配置覆盖缺省值，tool references 按模型推导。
pub fn get_anthropic_compat(model: &Model) -> AnthropicMessagesCompat {
    let compat = model.compat.as_ref();
    AnthropicMessagesCompat {
        supports_eager_tool_input_streaming: compat
            .and_then(|value| value.supports_eager_tool_input_streaming)
            .unwrap_or(true),
        supports_long_cache_retention: compat
            .and_then(|value| value.supports_long_cache_retention)
            .unwrap_or(true),
        send_session_affinity_headers: compat
            .and_then(|value| value.send_session_affinity_headers)
            .unwrap_or(false),
        supports_cache_control_on_tools: compat
            .and_then(|value| value.supports_cache_control_on_tools)
            .unwrap_or(true),
        supports_temperature: compat
            .and_then(|value| value.supports_temperature)
            .unwrap_or(true),
        force_adaptive_thinking: compat
            .and_then(|value| value.force_adaptive_thinking)
            .unwrap_or(false),
        allow_empty_signature: compat
            .and_then(|value| value.allow_empty_signature)
            .unwrap_or(false),
        supports_strict_tools: compat
            .and_then(|value| value.supports_strict_tools)
            .unwrap_or(false),
        supports_mid_convo_effort: false,
        supports_tool_references: default_supports_tool_references(model),
    }
}

/// 一方 Anthropic 模型(非 Haiku、非 Claude 3.x/4.0/4.1)支持 tool_reference。
fn default_supports_tool_references(model: &Model) -> bool {
    if model.provider != "anthropic" || model.id.contains("haiku") {
        return false;
    }
    // TS 正则 /^claude-(?:opus|sonnet|fable)-(\d+)(?:-(\d+))?(?:-|$)/
    let Some(rest) = model.id.strip_prefix("claude-") else {
        return false;
    };
    let mut segments = rest.splitn(3, '-');
    let family = segments.next().unwrap_or_default();
    if !matches!(family, "opus" | "sonnet" | "fable") {
        return false;
    }
    let Ok(major) = segments.next().unwrap_or_default().parse::<u32>() else {
        return false;
    };
    let minor = match segments.next() {
        None => 0,
        Some(raw) => {
            let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
            // 长度 ≥ 8 视为日期尾缀(如 20250929)→ 按 0 处理(对齐 TS)
            if digits.is_empty() || digits.len() >= 8 {
                0
            } else {
                digits.parse().unwrap_or(0)
            }
        }
    };
    major > 4 || (major == 4 && minor >= 5)
}

// ── 缓存控制 ─────────────────────────────────────────────────────────

/// TS resolveCacheRetention:显式选项优先,回退 PI_CACHE_RETENTION 环境值,
/// 默认 "short"。
fn resolve_cache_retention(options: Option<&SimpleStreamOptions>) -> CacheRetention {
    if let Some(retention) = options.and_then(|options| options.cache_retention) {
        return retention;
    }
    let env_value = || {
        std::env::var("PI_CACHE_RETENTION")
            .ok()
            .filter(|value| !value.is_empty())
    };
    let override_value = options
        .and_then(|options| options.env.as_ref())
        .and_then(|env| env.get("PI_CACHE_RETENTION"))
        .filter(|value| !value.is_empty())
        .cloned();
    let value = override_value.or_else(env_value);
    if value.as_deref() == Some("long") {
        CacheRetention::Long
    } else {
        CacheRetention::Short
    }
}

/// TS getCacheControl:none → 无标记;long + 支持时附 ttl "1h"。
fn get_cache_control(
    compat: &AnthropicMessagesCompat,
    options: Option<&SimpleStreamOptions>,
) -> (CacheRetention, Option<Value>) {
    let retention = resolve_cache_retention(options);
    if retention == CacheRetention::None {
        return (retention, None);
    }
    let mut cache_control = Map::new();
    cache_control.insert("type".to_string(), json!("ephemeral"));
    if retention == CacheRetention::Long && compat.supports_long_cache_retention {
        cache_control.insert("ttl".to_string(), json!("1h"));
    }
    (retention, Some(Value::Object(cache_control)))
}

// ── 上下文预算估算(TS utils/estimate.ts,与 openai 移植同源) ─────────

const CHARS_PER_TOKEN: i64 = 4;
const ESTIMATED_IMAGE_CHARS: i64 = 4800;
const CONTEXT_SAFETY_TOKENS: i64 = 4096;
const MIN_MAX_TOKENS: i64 = 1;

fn ceil_div4(chars: i64) -> i64 {
    (chars + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

fn message_timestamp(message: &Message) -> i64 {
    match message {
        Message::User(user) => user.timestamp,
        Message::Assistant(assistant) => assistant.timestamp,
        Message::ToolResult(result) => result.timestamp,
    }
}

fn text_or_image_chars(content: &[TextOrImageContent]) -> i64 {
    content
        .iter()
        .map(|block| match block {
            TextOrImageContent::Text { text, .. } => text.chars().count() as i64,
            TextOrImageContent::Image { .. } => ESTIMATED_IMAGE_CHARS,
        })
        .sum()
}

fn estimate_message_tokens(message: &Message) -> i64 {
    let chars = match message {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.chars().count() as i64,
            UserContent::Blocks(blocks) => text_or_image_chars(blocks),
        },
        Message::ToolResult(result) => text_or_image_chars(&result.content),
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .map(|block| match block {
                AssistantContent::Text { text, .. } => text.chars().count() as i64,
                AssistantContent::Thinking { thinking, .. } => thinking.chars().count() as i64,
                AssistantContent::ToolCall(tool_call) => {
                    tool_call.name.chars().count() as i64
                        + serde_json::to_string(&tool_call.arguments)
                            .map(|serialized| serialized.chars().count() as i64)
                            .unwrap_or(0)
                }
            })
            .sum(),
    };
    ceil_div4(chars)
}

fn estimate_tools_tokens<'a>(tools: impl Iterator<Item = &'a Tool>) -> i64 {
    let list: Vec<&Tool> = tools.collect();
    if list.is_empty() {
        return 0;
    }
    let serialized = serde_json::to_string(&list).unwrap_or_default();
    ceil_div4(serialized.chars().count() as i64)
}

fn estimate_context_tokens(context: &Context) -> i64 {
    let mut latest_prefix_timestamp = i64::MIN;
    let mut usage_info: Option<(i64, usize)> = None;
    for (index, message) in context.messages.iter().enumerate() {
        if let Message::Assistant(assistant) = message {
            let applies_to_prefix = assistant.timestamp >= latest_prefix_timestamp;
            if applies_to_prefix
                && assistant.stop_reason != StopReason::Aborted
                && assistant.stop_reason != StopReason::Error
            {
                let usage = &assistant.usage;
                let total = if usage.total_tokens > 0 {
                    usage.total_tokens
                } else {
                    usage.input + usage.output + usage.cache_read + usage.cache_write
                };
                if total > 0 {
                    usage_info = Some((total, index));
                }
            }
        }
        latest_prefix_timestamp = latest_prefix_timestamp.max(message_timestamp(message));
    }

    match usage_info {
        Some((usage_tokens, index)) => {
            let trailing: i64 = context.messages[index + 1..]
                .iter()
                .map(estimate_message_tokens)
                .sum();
            let mut added_names: HashSet<&str> = HashSet::new();
            for message in &context.messages[index + 1..] {
                if let Message::ToolResult(result) = message {
                    if let Some(names) = &result.added_tool_names {
                        for name in names {
                            added_names.insert(name.as_str());
                        }
                    }
                }
            }
            let added_tool_tokens = estimate_tools_tokens(
                context
                    .tools
                    .iter()
                    .filter(|tool| added_names.contains(tool.name.as_str())),
            );
            usage_tokens + trailing + added_tool_tokens
        }
        None => {
            let messages: i64 = context.messages.iter().map(estimate_message_tokens).sum();
            let system = context
                .system_prompt
                .as_ref()
                .map(|prompt| ceil_div4(prompt.chars().count() as i64))
                .unwrap_or(0);
            let tools = estimate_tools_tokens(context.tools.iter());
            messages + system + tools
        }
    }
}

fn clamp_max_tokens_to_context(model: &Model, context: &Context, max_tokens: i64) -> i64 {
    if model.context_window <= 0 {
        return max_tokens.max(MIN_MAX_TOKENS);
    }
    let available = model.context_window - estimate_context_tokens(context) - CONTEXT_SAFETY_TOKENS;
    max_tokens.min(available.max(MIN_MAX_TOKENS))
}

// ── thinking 预算(TS api/simple-options.ts) ─────────────────────────

fn clamp_reasoning(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    }
}

fn thinking_budget_for_level(level: ThinkingLevel, custom: Option<&ThinkingBudgets>) -> i64 {
    let level = clamp_reasoning(level);
    let default_budget = match level {
        ThinkingLevel::Minimal => DEFAULT_THINKING_BUDGET_MINIMAL,
        ThinkingLevel::Low => DEFAULT_THINKING_BUDGET_LOW,
        ThinkingLevel::Medium => DEFAULT_THINKING_BUDGET_MEDIUM,
        _ => DEFAULT_THINKING_BUDGET_HIGH,
    };
    let custom_budget = custom.and_then(|budgets| match level {
        ThinkingLevel::Minimal => budgets.minimal,
        ThinkingLevel::Low => budgets.low,
        ThinkingLevel::Medium => budgets.medium,
        ThinkingLevel::High => budgets.high,
        _ => None,
    });
    custom_budget.map(i64::from).unwrap_or(default_budget)
}

fn clamp_thinking_budget_to_answer_room(thinking_budget: i64, ceiling: i64) -> i64 {
    thinking_budget.min((ceiling - MIN_ANSWER_TOKENS).max(0))
}

/// TS adjustMaxTokensForThinking:baseMaxTokens None 时用模型上限容纳 thinking。
fn adjust_max_tokens_for_thinking(
    base_max_tokens: Option<i64>,
    model_max_tokens: i64,
    level: ThinkingLevel,
    custom_budgets: Option<&ThinkingBudgets>,
) -> (i64, i64) {
    let mut thinking_budget = thinking_budget_for_level(level, custom_budgets);
    let max_tokens = match base_max_tokens {
        None => model_max_tokens,
        Some(base) => (base + thinking_budget).min(model_max_tokens),
    };
    if max_tokens <= thinking_budget {
        thinking_budget = clamp_thinking_budget_to_answer_room(thinking_budget, max_tokens);
    }
    (max_tokens, thinking_budget)
}

/// TS streamSimple 的选项映射(Rust 合并了 StreamOptions/SimpleStreamOptions)。
struct ResolvedStreamOptions {
    max_tokens: i64,
    temperature: Option<f64>,
    thinking_enabled: bool,
    /// 仅 budget-based thinking(adaptive 路径未实现,见模块注释)。
    thinking_budget_tokens: Option<i64>,
}

fn resolve_stream_options(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> ResolvedStreamOptions {
    let temperature = options.and_then(|options| options.temperature);
    let base_max_tokens = clamp_max_tokens_to_context(
        model,
        context,
        options
            .and_then(|o| o.max_tokens)
            .map(i64::from)
            .unwrap_or(model.max_tokens),
    );
    let Some(level) = options.and_then(|options| options.reasoning) else {
        return ResolvedStreamOptions {
            max_tokens: base_max_tokens,
            temperature,
            thinking_enabled: false,
            thinking_budget_tokens: None,
        };
    };
    let (adjusted_max, budget) = adjust_max_tokens_for_thinking(
        Some(base_max_tokens),
        model.max_tokens,
        level,
        options.and_then(|options| options.thinking_budgets.as_ref()),
    );
    let max_tokens = clamp_max_tokens_to_context(model, context, adjusted_max);
    let budget = budget.min((max_tokens - MIN_ANSWER_TOKENS).max(0));
    ResolvedStreamOptions {
        max_tokens,
        temperature,
        thinking_enabled: true,
        thinking_budget_tokens: Some(budget),
    }
}

// ── 消息变换(TS api/transform-messages.ts) ──────────────────────────

fn replace_images_with_placeholder(
    content: &[TextOrImageContent],
    placeholder: &str,
) -> Vec<TextOrImageContent> {
    let mut result: Vec<TextOrImageContent> = Vec::new();
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            TextOrImageContent::Image { .. } => {
                if !previous_was_placeholder {
                    result.push(TextOrImageContent::text(placeholder));
                }
                previous_was_placeholder = true;
            }
            TextOrImageContent::Text {
                text,
                text_signature,
            } => {
                result.push(TextOrImageContent::Text {
                    text: text.clone(),
                    text_signature: text_signature.clone(),
                });
                previous_was_placeholder = text == placeholder;
            }
        }
    }
    result
}

/// Anthropic 要求 id 匹配 ^[a-zA-Z0-9_-]+$ 且 ≤64 字符。
fn normalize_tool_call_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sanitized.chars().take(64).collect()
}

/// TS transformMessages:非图片模型降级图片;跨模型 replay 清理 thinking/签名与
/// tool call id(normalizeId 回调);孤儿 tool call 合成错误结果;
/// error/aborted assistant 整条丢弃。
fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_id: impl Fn(&str) -> String,
) -> Vec<Message> {
    let supports_images = model.input.contains(&InputKind::Image);
    let mut tool_call_id_map: HashMap<String, String> = HashMap::new();

    let mut first_pass: Vec<Message> = Vec::with_capacity(messages.len());
    for message in messages {
        match message {
            Message::User(user) => {
                let mut user = user.clone();
                if !supports_images {
                    if let UserContent::Blocks(blocks) = &mut user.content {
                        *blocks = replace_images_with_placeholder(
                            blocks,
                            NON_VISION_USER_IMAGE_PLACEHOLDER,
                        );
                    }
                }
                first_pass.push(Message::User(user));
            }
            Message::ToolResult(result) => {
                let mut result = result.clone();
                if !supports_images {
                    result.content = replace_images_with_placeholder(
                        &result.content,
                        NON_VISION_TOOL_IMAGE_PLACEHOLDER,
                    );
                }
                if let Some(normalized) = tool_call_id_map.get(&result.tool_call_id) {
                    if *normalized != result.tool_call_id {
                        result.tool_call_id = normalized.clone();
                    }
                }
                first_pass.push(Message::ToolResult(result));
            }
            Message::Assistant(assistant) => {
                let mut assistant = assistant.clone();
                let is_same_model = assistant.provider == model.provider
                    && assistant.api == model.api
                    && assistant.model == model.id;
                let mut content = Vec::with_capacity(assistant.content.len());
                for block in std::mem::take(&mut assistant.content) {
                    match block {
                        AssistantContent::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            if redacted {
                                // redacted 载荷仅同模型有效,跨模型丢弃
                                if is_same_model {
                                    content.push(AssistantContent::Thinking {
                                        thinking,
                                        thinking_signature,
                                        redacted,
                                    });
                                }
                            } else {
                                let has_signature = thinking_signature
                                    .as_ref()
                                    .is_some_and(|signature| !signature.is_empty());
                                if is_same_model && has_signature {
                                    // 同模型带签名:即使 thinking 文本为空也保留(replay 需要)
                                    content.push(AssistantContent::Thinking {
                                        thinking,
                                        thinking_signature,
                                        redacted,
                                    });
                                } else if thinking.trim().is_empty() {
                                    // 空 thinking 丢弃
                                } else if is_same_model {
                                    content.push(AssistantContent::Thinking {
                                        thinking,
                                        thinking_signature,
                                        redacted,
                                    });
                                } else {
                                    // 跨模型:thinking 转纯文本
                                    content.push(AssistantContent::text(thinking));
                                }
                            }
                        }
                        AssistantContent::Text {
                            text,
                            text_signature,
                        } => {
                            content.push(if is_same_model {
                                AssistantContent::Text {
                                    text,
                                    text_signature,
                                }
                            } else {
                                AssistantContent::Text {
                                    text,
                                    text_signature: None,
                                }
                            });
                        }
                        AssistantContent::ToolCall(mut tool_call) => {
                            if !is_same_model {
                                tool_call.thought_signature = None;
                                let normalized = normalize_id(&tool_call.id);
                                if normalized != tool_call.id {
                                    tool_call_id_map
                                        .insert(tool_call.id.clone(), normalized.clone());
                                    tool_call.id = normalized;
                                }
                            }
                            content.push(AssistantContent::ToolCall(tool_call));
                        }
                    }
                }
                assistant.content = content;
                first_pass.push(Message::Assistant(assistant));
            }
        }
    }

    // 第二遍:孤儿 tool call 补合成错误结果(TS insertSyntheticToolResults)
    let mut result: Vec<Message> = Vec::new();
    let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
    let mut existing_tool_result_ids: HashSet<String> = HashSet::new();
    macro_rules! flush_pending {
        () => {
            for tool_call in pending_tool_calls.drain(..) {
                if !existing_tool_result_ids.contains(&tool_call.id) {
                    result.push(Message::ToolResult(ToolResultMessage {
                        role: "toolResult".to_string(),
                        tool_call_id: tool_call.id,
                        tool_name: tool_call.name,
                        content: vec![TextOrImageContent::text(SYNTHETIC_TOOL_RESULT_TEXT)],
                        details: None,
                        usage: None,
                        added_tool_names: None,
                        is_error: true,
                        timestamp: now_ts_nanos() / 1_000_000,
                    }));
                }
            }
            existing_tool_result_ids.clear();
        };
    }

    for message in first_pass {
        match message {
            Message::Assistant(assistant) => {
                flush_pending!();
                if assistant.stop_reason == StopReason::Error
                    || assistant.stop_reason == StopReason::Aborted
                {
                    continue;
                }
                let tool_calls: Vec<ToolCall> = assistant
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AssistantContent::ToolCall(tool_call) => Some(tool_call.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls;
                    existing_tool_result_ids.clear();
                }
                result.push(Message::Assistant(assistant));
            }
            Message::ToolResult(tool_result) => {
                existing_tool_result_ids.insert(tool_result.tool_call_id.clone());
                result.push(Message::ToolResult(tool_result));
            }
            Message::User(user) => {
                flush_pending!();
                result.push(Message::User(user));
            }
        }
    }
    flush_pending!();
    result
}

// ── 消息序列化(TS convertMessages / convertToolResult) ──────────────

fn image_source_block(data: &str, mime_type: &str) -> Value {
    json!({
        "type": "image",
        "source": { "type": "base64", "media_type": mime_type, "data": data },
    })
}

/// TS convertContentBlocks:纯文本 → 拼接字符串;含图片 → 块数组(纯图片补占位文本)。
enum ConvertedContent {
    Text(String),
    Blocks(Vec<Value>),
}

fn convert_content_blocks(content: &[TextOrImageContent]) -> ConvertedContent {
    let has_images = content
        .iter()
        .any(|block| matches!(block, TextOrImageContent::Image { .. }));
    if !has_images {
        let text = content
            .iter()
            .filter_map(|block| match block {
                TextOrImageContent::Text { text, .. } => Some(text.as_str()),
                TextOrImageContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return ConvertedContent::Text(text);
    }
    let mut blocks: Vec<Value> = Vec::new();
    for block in content {
        match block {
            TextOrImageContent::Text { text, .. } => {
                blocks.push(json!({ "type": "text", "text": text }));
            }
            TextOrImageContent::Image { data, mime_type } => {
                blocks.push(image_source_block(data, mime_type));
            }
        }
    }
    if !blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("text"))
    {
        blocks.insert(0, json!({ "type": "text", "text": IMAGE_ONLY_TEXT }));
    }
    ConvertedContent::Blocks(blocks)
}

fn system_text_block(text: &str, cache_control: Option<&Value>) -> Value {
    let mut block = Map::new();
    block.insert("type".to_string(), json!("text"));
    block.insert("text".to_string(), json!(text));
    if let Some(cache_control) = cache_control {
        block.insert("cache_control".to_string(), cache_control.clone());
    }
    Value::Object(block)
}

struct ConvertedToolResult {
    tool_result: Value,
    sibling_content: Vec<Value>,
}

fn convert_tool_result(
    result: &ToolResultMessage,
    is_oauth: bool,
    deferred_tool_names: &HashSet<String>,
    loaded_tool_names: &mut HashSet<String>,
    normalize_tool_name: &impl Fn(&str) -> String,
) -> ConvertedToolResult {
    let mut references: Vec<Value> = Vec::new();
    for name in result.added_tool_names.iter().flatten() {
        let normalized = normalize_tool_name(name);
        if !deferred_tool_names.contains(&normalized) || loaded_tool_names.contains(&normalized) {
            continue;
        }
        loaded_tool_names.insert(normalized);
        let wire_name = if is_oauth {
            to_claude_code_name(name)
        } else {
            name.clone()
        };
        references.push(json!({ "type": "tool_reference", "tool_name": wire_name }));
    }
    let converted = convert_content_blocks(&result.content);
    let (content, sibling_content) = if references.is_empty() {
        let content = match converted {
            ConvertedContent::Text(text) => Value::String(text),
            ConvertedContent::Blocks(blocks) => Value::Array(blocks),
        };
        (content, Vec::new())
    } else {
        // Anthropic 拒绝 tool_reference 与普通 tool-result 内容混排:内容平移为兄弟块
        let sibling = match converted {
            ConvertedContent::Text(text) => vec![json!({ "type": "text", "text": text })],
            ConvertedContent::Blocks(blocks) => blocks,
        };
        (Value::Array(references), sibling)
    };
    ConvertedToolResult {
        tool_result: json!({
            "type": "tool_result",
            "tool_use_id": result.tool_call_id,
            "content": content,
            "is_error": result.is_error,
        }),
        sibling_content,
    }
}

fn convert_messages(
    transformed: &[Message],
    is_oauth: bool,
    cache_control: Option<&Value>,
    allow_empty_signature: bool,
    deferred_tool_names: &HashSet<String>,
    normalize_tool_name: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    let mut loaded_tool_names: HashSet<String> = HashSet::new();
    let mut index = 0usize;
    while index < transformed.len() {
        let message = &transformed[index];
        match message {
            Message::User(user) => {
                match &user.content {
                    UserContent::Text(text) => {
                        if !text.trim().is_empty() {
                            params.push(json!({ "role": "user", "content": text }));
                        }
                    }
                    UserContent::Blocks(blocks) => {
                        let mut converted: Vec<Value> = Vec::new();
                        for block in blocks {
                            match block {
                                TextOrImageContent::Text { text, .. } => {
                                    converted.push(json!({ "type": "text", "text": text }));
                                }
                                TextOrImageContent::Image { data, mime_type } => {
                                    converted.push(image_source_block(data, mime_type));
                                }
                            }
                        }
                        converted.retain(|block| {
                            block.get("type").and_then(Value::as_str) != Some("text")
                                || !block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .trim()
                                    .is_empty()
                        });
                        if converted.is_empty() {
                            index += 1;
                            continue;
                        }
                        params.push(json!({ "role": "user", "content": converted }));
                    }
                }
                index += 1;
            }
            Message::Assistant(assistant) => {
                let mut blocks: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantContent::Text { text, .. } => {
                            if text.trim().is_empty() {
                                continue;
                            }
                            blocks.push(json!({ "type": "text", "text": text }));
                        }
                        AssistantContent::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            if *redacted {
                                // redacted thinking:密文载荷原样回放
                                let mut redacted_block = Map::new();
                                redacted_block
                                    .insert("type".to_string(), json!("redacted_thinking"));
                                if let Some(data) = thinking_signature {
                                    redacted_block.insert("data".to_string(), json!(data));
                                }
                                blocks.push(Value::Object(redacted_block));
                                continue;
                            }
                            let has_signature = thinking_signature
                                .as_deref()
                                .is_some_and(|signature| !signature.trim().is_empty());
                            if thinking.trim().is_empty() && !has_signature {
                                continue;
                            }
                            if !has_signature {
                                // 签名缺失(如中止流):默认转纯文本,compat 标记的模型保留空签名
                                if allow_empty_signature {
                                    blocks.push(json!({
                                        "type": "thinking",
                                        "thinking": thinking,
                                        "signature": "",
                                    }));
                                } else {
                                    blocks.push(json!({ "type": "text", "text": thinking }));
                                }
                            } else {
                                blocks.push(json!({
                                    "type": "thinking",
                                    "thinking": thinking,
                                    "signature": thinking_signature.clone().unwrap_or_default(),
                                }));
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            let name = if is_oauth {
                                to_claude_code_name(&tool_call.name)
                            } else {
                                tool_call.name.clone()
                            };
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": tool_call.id,
                                "name": name,
                                "input": tool_call.arguments,
                            }));
                        }
                    }
                }
                if blocks.is_empty() {
                    index += 1;
                    continue;
                }
                params.push(json!({ "role": "assistant", "content": blocks }));
                index += 1;
            }
            Message::ToolResult(_) => {
                // 连续 toolResult 合并为一条 user 消息(z.ai Anthropic 端点需要)
                let mut tool_results: Vec<Value> = Vec::new();
                let mut sibling_content: Vec<Value> = Vec::new();
                let mut cursor = index;
                while let Some(Message::ToolResult(result)) = transformed.get(cursor) {
                    let converted = convert_tool_result(
                        result,
                        is_oauth,
                        deferred_tool_names,
                        &mut loaded_tool_names,
                        normalize_tool_name,
                    );
                    tool_results.push(converted.tool_result);
                    sibling_content.extend(converted.sibling_content);
                    cursor += 1;
                }
                index = cursor;
                tool_results.extend(sibling_content);
                params.push(json!({ "role": "user", "content": tool_results }));
                continue;
            }
        }
    }

    // 末条 user 消息的最后一个块加 cache_control,缓存对话历史
    if let (Some(cache_control), Some(last)) = (cache_control, params.last_mut()) {
        if last.get("role").and_then(Value::as_str) == Some("user") {
            match last.get_mut("content") {
                Some(Value::Array(blocks)) => {
                    if let Some(last_block) = blocks.last_mut() {
                        let kind = last_block
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if matches!(kind, "text" | "image" | "tool_result") {
                            if let Some(object) = last_block.as_object_mut() {
                                object.insert("cache_control".to_string(), cache_control.clone());
                            }
                        }
                    }
                }
                Some(Value::String(text)) => {
                    let content = json!([{
                        "type": "text",
                        "text": text,
                        "cache_control": cache_control,
                    }]);
                    if let Some(object) = last.as_object_mut() {
                        object.insert("content".to_string(), content);
                    }
                }
                _ => {}
            }
        }
    }

    params
}

// ── 延迟加载工具拆分(TS utils/deferred-tools.ts) ────────────────────

fn split_deferred_tools(
    tools: &[Tool],
    messages: &[Message],
    enabled: bool,
    normalize_tool_name: &impl Fn(&str) -> String,
) -> (Vec<Tool>, Vec<Tool>) {
    // 有序去重:同名保留首次插入位置、值取后者(TS Map.set 语义)
    let mut ordered: Vec<(String, Tool)> = Vec::new();
    for tool in tools {
        let name = normalize_tool_name(&tool.name);
        if let Some(slot) = ordered.iter_mut().find(|(existing, _)| *existing == name) {
            slot.1 = tool.clone();
        } else {
            ordered.push((name, tool.clone()));
        }
    }
    if !enabled {
        return (
            ordered.into_iter().map(|(_, tool)| tool).collect(),
            Vec::new(),
        );
    }

    let mut used_names: HashSet<String> = HashSet::new();
    for message in messages {
        if let Message::Assistant(assistant) = message {
            for block in &assistant.content {
                if let AssistantContent::ToolCall(tool_call) = block {
                    used_names.insert(normalize_tool_name(&tool_call.name));
                }
            }
        }
    }
    let mut deferred_names: HashSet<String> = HashSet::new();
    for message in messages {
        if let Message::ToolResult(result) = message {
            for name in result.added_tool_names.iter().flatten() {
                let normalized = normalize_tool_name(name);
                if !used_names.contains(&normalized) {
                    deferred_names.insert(normalized);
                }
            }
        }
    }
    let mut immediate = Vec::new();
    let mut deferred = Vec::new();
    for (name, tool) in ordered {
        if deferred_names.contains(&name) {
            deferred.push(tool);
        } else {
            immediate.push(tool);
        }
    }
    (immediate, deferred)
}

// ── tools 序列化(TS convertTools) ───────────────────────────────────

/// strict 工具(constrainedSampling)未建模 → strict 恒不启用,
/// input_schema 恒为 legacy `{type:"object",properties,required}` 形状。
fn convert_tools(
    tools: &[Tool],
    is_oauth: bool,
    supports_eager_tool_input_streaming: bool,
    cache_control: Option<&Value>,
    defer_loading: bool,
) -> Vec<Value> {
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let properties = tool
                .parameters
                .get("properties")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let required = tool
                .parameters
                .get("required")
                .cloned()
                .unwrap_or_else(|| json!([]));
            let name = if is_oauth {
                to_claude_code_name(&tool.name)
            } else {
                tool.name.clone()
            };
            let mut entry = Map::new();
            entry.insert("name".to_string(), json!(name));
            entry.insert("description".to_string(), json!(tool.description));
            if supports_eager_tool_input_streaming {
                entry.insert("eager_input_streaming".to_string(), json!(true));
            }
            entry.insert(
                "input_schema".to_string(),
                json!({ "type": "object", "properties": properties, "required": required }),
            );
            if defer_loading {
                entry.insert("defer_loading".to_string(), json!(true));
            }
            if let Some(cache_control) = cache_control {
                if index + 1 == tools.len() {
                    entry.insert("cache_control".to_string(), cache_control.clone());
                }
            }
            Value::Object(entry)
        })
        .collect()
}

// ── beta 特性(TS getBetaFeatures) ───────────────────────────────────

/// betas 经 `anthropic-beta` 请求头传输(SDK 行为),不进请求体。
fn get_beta_features(
    model: &Model,
    context: &Context,
    is_oauth: bool,
    thinking_enabled: bool,
    options: Option<&SimpleStreamOptions>,
) -> Vec<String> {
    // model.headers 后 options.headers,后者覆盖同名配置
    let mut configured: Option<String> = None;
    for headers in [
        model.headers.as_ref(),
        options.and_then(|o| o.headers.as_ref()),
    ] {
        let Some(headers) = headers else { continue };
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("anthropic-beta") {
                configured = Some(value.clone());
            }
        }
    }
    if let Some(features) = configured {
        let mut seen: HashSet<String> = HashSet::new();
        let mut parsed = Vec::new();
        for feature in features.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            if seen.insert(feature.to_string()) {
                parsed.push(feature.to_string());
            }
        }
        return parsed;
    }

    let compat = get_anthropic_compat(model);
    let mut features: Vec<String> = Vec::new();
    let push = |feature: &str, features: &mut Vec<String>| {
        if !features.iter().any(|existing| existing == feature) {
            features.push(feature.to_string());
        }
    };
    if is_oauth {
        push("claude-code-20250219", &mut features);
        push("oauth-2025-04-20", &mut features);
    }
    // 不支持 eager 工具入参流式时,退回 fine-grained tool streaming beta
    if !context.tools.is_empty() && !compat.supports_eager_tool_input_streaming {
        push(FINE_GRAINED_TOOL_STREAMING_BETA, &mut features);
    }
    if model.reasoning && thinking_enabled && !compat.force_adaptive_thinking {
        push(INTERLEAVED_THINKING_BETA, &mut features);
    }
    features
}

// ── 认证与请求头 ─────────────────────────────────────────────────────

/// TS assertRequestAuth:显式 apiKey 优先;否则看 headers 是否自带鉴权(None),
/// 都没有则报错。
fn resolve_api_key(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
) -> Result<Option<String>, String> {
    if let Some(api_key) = options.and_then(|options| options.api_key.clone()) {
        return Ok(Some(api_key));
    }
    let has_auth = |headers: Option<&HashMap<String, String>>| {
        headers.is_some_and(|headers| {
            headers.iter().any(|(key, value)| {
                (key.eq_ignore_ascii_case("authorization")
                    || key.eq_ignore_ascii_case("x-api-key")
                    || key.eq_ignore_ascii_case("cf-aig-authorization"))
                    && !value.trim().is_empty()
            })
        })
    };
    if has_auth(model.headers.as_ref()) || has_auth(options.and_then(|o| o.headers.as_ref())) {
        return Ok(None);
    }
    Err(format!("No API key for provider: {}", model.provider))
}

fn push_custom_headers(source: Option<&HashMap<String, String>>, headers: &mut HeaderMap) {
    let Some(source) = source else { return };
    for (key, value) in source {
        match (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(value)) => {
                headers.insert(name, value);
            }
            _ => eprintln!("[agent/llm] 忽略非法自定义 header: {key}"),
        }
    }
}

/// 请求头组装:SDK 默认头在前,auth 次之,model.headers / options.headers 覆盖同名。
fn build_request_headers(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
    api_key: Option<&str>,
    is_oauth: bool,
    session_affinity: Option<&str>,
    beta_features: &[String],
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let mut insert = |name: &str, value: &str| {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        }
    };
    insert("content-type", "application/json");
    insert("accept", "application/json");
    insert("user-agent", &user_agent());
    insert("anthropic-version", ANTHROPIC_VERSION);
    insert("anthropic-dangerous-direct-browser-access", "true");
    if is_oauth {
        // OAuth token 仅接受 Claude Code 客户端身份,保留伪装 UA 不换 pi-repomeow
        insert("user-agent", &format!("claude-cli/{CLAUDE_CODE_VERSION}"));
        insert("x-app", "cli");
    }
    if let Some(session) = session_affinity {
        insert("x-session-affinity", session);
    }
    if !beta_features.is_empty() {
        insert("anthropic-beta", &beta_features.join(","));
    }
    if let Some(api_key) = api_key {
        if is_oauth || model.provider == "github-copilot" {
            insert("authorization", &format!("Bearer {api_key}"));
        } else {
            insert("x-api-key", api_key);
        }
    }
    push_custom_headers(model.headers.as_ref(), &mut headers);
    push_custom_headers(
        options.and_then(|options| options.headers.as_ref()),
        &mut headers,
    );
    headers
}

/// SDK 行为:POST `{baseUrl}/v1/messages`。
fn request_url(model: &Model) -> String {
    format!("{}/v1/messages", model.base_url.trim_end_matches('/'))
}

fn build_http_client(options: Option<&SimpleStreamOptions>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(15));
    if let Some(timeout_ms) = options.and_then(|options| options.timeout_ms) {
        builder = builder.timeout(Duration::from_millis(timeout_ms));
    }
    builder
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))
}

/// HTTP 非 2xx 响应 → "status: message"(对齐 SDK APIError 的 message 形状)。
fn format_http_error(status: u16, body: &str) -> String {
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) {
        if let Some(message) = map
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            let message = message.trim();
            if !message.is_empty() {
                return format!("{status}: {message}");
            }
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        status.to_string()
    } else {
        let truncated: String = trimmed.chars().take(512).collect();
        format!("{status}: {truncated}")
    }
}

fn headers_to_map(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for (name, value) in headers {
        let entry = map.entry(name.as_str().to_string()).or_default();
        if !entry.is_empty() {
            entry.push_str(", ");
        }
        entry.push_str(value.to_str().unwrap_or_default());
    }
    map
}

// ── provider 内层重试(TS utils/provider-retry.ts) ───────────────────

/// TS retryProviderRequest 裸默认为 0;应用侧对齐任务规格:未显式配置时重试 2 次。
const DEFAULT_MAX_RETRIES: u32 = 2;
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// 单次请求失败的 provider 语义信息(对齐 TS ProviderError 的 status/headers)。
struct ProviderRequestError {
    message: String,
    /// None = 传输层失败(连接/读超时等,视为可重试)。
    status: Option<u16>,
    headers: Option<HeaderMap>,
}

enum RetryDecision {
    /// 等待指定毫秒后重发。
    Retry(u64),
    /// 服务端要求的延迟超过上限:立即失败(带对齐 TS 的文案)。
    DelayExceeded(String),
    /// 不可重试(重试次数耗尽或状态码/header 判定)。
    NotRetryable,
}

/// 对齐 SDK 固定重试策略:x-should-retry 头优先,其次 408/409/429/5xx,
/// 无 status(传输错误)视为可重试。
fn is_retryable_provider_error(error: &ProviderRequestError) -> bool {
    let should_retry = error
        .headers
        .as_ref()
        .and_then(|headers| headers.get("x-should-retry"))
        .and_then(|value| value.to_str().ok());
    if should_retry == Some("true") {
        return true;
    }
    if should_retry == Some("false") {
        return false;
    }
    match error.status {
        None => true,
        Some(status) => status == 408 || status == 409 || status == 429 || status >= 500,
    }
}

/// TS Number.parseFloat 的宽松前缀语义("120ms" → 120)。
fn parse_float_prefix(text: &str) -> Option<f64> {
    let trimmed = text.trim_start();
    let end = trimmed
        .find(|c: char| {
            !c.is_ascii_digit() && c != '.' && c != 'e' && c != 'E' && c != '+' && c != '-'
        })
        .unwrap_or(trimmed.len());
    trimmed[..end].parse::<f64>().ok()
}

/// HTTP 日期 retry-after → 距今毫秒(过去时刻 ≤ 0,由调用方钳制);不可解析 → 0。
fn http_date_delay_ms(value: &str) -> f64 {
    let parsed = chrono::DateTime::parse_from_rfc2822(value.trim()).ok();
    match parsed {
        Some(date) => {
            (date.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_milliseconds() as f64
        }
        None => 0.0,
    }
}

/// 退避抖动用伪随机数(TS 为 Math.random);时间派生对退避足够。
fn pseudo_random_unit() -> f64 {
    (now_ts_nanos() % 1_000_003) as f64 / 1_000_003.0
}

/// 服务端延迟超上限 → Err(立即失败文案);否则返回生效延迟(下限 0)。
fn validate_server_retry_delay(
    delay_ms: f64,
    max_retry_delay_ms: u64,
    provider_error_message: &str,
) -> Result<u64, String> {
    if max_retry_delay_ms > 0 && delay_ms > max_retry_delay_ms as f64 {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s). {provider_error_message}",
            (delay_ms / 1000.0).ceil() as i64,
            (max_retry_delay_ms as f64 / 1000.0).ceil() as i64,
        ));
    }
    Ok(delay_ms.max(0.0) as u64)
}

/// TS getRetryDelayMs:retry-after-ms → retry-after(秒/HTTP 日期)→ 指数退避
/// `min(0.5·2^retryIndex, 8)s × (1 - rand·0.25)`。
fn get_retry_delay_ms(
    error: &ProviderRequestError,
    retry_index: u32,
    max_retry_delay_ms: u64,
) -> Result<u64, String> {
    let headers = error.headers.as_ref();
    if let Some(value) = headers
        .and_then(|headers| headers.get("retry-after-ms"))
        .and_then(|value| value.to_str().ok())
    {
        if let Some(delay_ms) = parse_float_prefix(value) {
            return validate_server_retry_delay(delay_ms, max_retry_delay_ms, &error.message);
        }
    }
    if let Some(value) = headers
        .and_then(|headers| headers.get("retry-after"))
        .and_then(|value| value.to_str().ok())
    {
        let delay_ms = match parse_float_prefix(value) {
            Some(seconds) => seconds * 1000.0,
            None => http_date_delay_ms(value),
        };
        return validate_server_retry_delay(delay_ms, max_retry_delay_ms, &error.message);
    }
    let exponential_delay = (0.5 * 2f64.powi(retry_index as i32)).min(8.0) * 1000.0;
    Ok((exponential_delay * (1.0 - pseudo_random_unit() * 0.25)) as u64)
}

/// TS retryProviderRequest 的单轮判定:耗尽或不可重试 → NotRetryable。
fn retry_decision(
    error: &ProviderRequestError,
    retry_index: u32,
    retries_remaining: u32,
    max_retry_delay_ms: u64,
) -> RetryDecision {
    if retries_remaining == 0 || !is_retryable_provider_error(error) {
        return RetryDecision::NotRetryable;
    }
    match get_retry_delay_ms(error, retry_index, max_retry_delay_ms) {
        Ok(delay_ms) => RetryDecision::Retry(delay_ms),
        Err(message) => RetryDecision::DelayExceeded(message),
    }
}

// ── 容错 JSON(TS utils/json-parse.ts,与 openai 移植同源) ───────────

fn escape_control_character(ch: char) -> String {
    match ch {
        '\u{8}' => "\\b".to_string(),
        '\u{c}' => "\\f".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        _ => format!("\\u{:04x}", ch as u32),
    }
}

/// TS repairJson:转义字符串内的裸控制字符、把非法转义的双重化。
fn repair_json(json_text: &str) -> String {
    let mut repaired = String::with_capacity(json_text.len());
    let chars: Vec<char> = json_text.chars().collect();
    let mut in_string = false;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if !in_string {
            repaired.push(ch);
            if ch == '"' {
                in_string = true;
            }
            index += 1;
            continue;
        }
        if ch == '"' {
            repaired.push(ch);
            in_string = false;
            index += 1;
            continue;
        }
        if ch == '\\' {
            match chars.get(index + 1) {
                None => {
                    repaired.push_str("\\\\");
                    index += 1;
                }
                Some('u') => {
                    let hex: String = chars
                        .get(index + 2..index + 6)
                        .unwrap_or(&[])
                        .iter()
                        .collect();
                    if hex.chars().count() == 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        repaired.push_str("\\u");
                        repaired.push_str(&hex);
                        index += 6;
                    } else {
                        repaired.push_str("\\\\");
                        index += 1;
                    }
                }
                Some(next) if matches!(next, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') => {
                    repaired.push('\\');
                    repaired.push(*next);
                    index += 2;
                }
                Some(_) => {
                    repaired.push_str("\\\\");
                    index += 1;
                }
            }
            continue;
        }
        if (ch as u32) < 0x20 {
            repaired.push_str(&escape_control_character(ch));
        } else {
            repaired.push(ch);
        }
        index += 1;
    }
    repaired
}

#[derive(Clone, Copy)]
enum JsonFrame {
    Object,
    Array,
}

/// 扫描部分 JSON,生成「补全闭合」候选并返回首个可解析者。
fn close_partial_json(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut stack: Vec<JsonFrame> = Vec::new();
    let mut in_string = false;
    let mut string_start = 0usize;
    let mut escaped = false;

    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else {
                match byte {
                    b'\\' => escaped = true,
                    b'"' => in_string = false,
                    _ => {}
                }
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                string_start = index;
            }
            b'{' => stack.push(JsonFrame::Object),
            b'[' => stack.push(JsonFrame::Array),
            b'}' | b']' => {
                stack.pop();
            }
            _ => {}
        }
        index += 1;
    }

    let mut candidates: Vec<String> = Vec::new();
    if in_string {
        let mut kept = input.to_string();
        if escaped {
            kept.pop();
        }
        kept.push('"');
        candidates.push(kept);
        candidates.push(input[..string_start].to_string());
    } else {
        candidates.push(input.to_string());
        let cut = input
            .rfind(['{', '}', '[', ']', '"', ':', ','])
            .map_or(0, |position| position + 1);
        candidates.push(input[..cut].to_string());
    }

    for candidate in candidates {
        if let Some(closed) = close_candidate(candidate, &stack) {
            if serde_json::from_str::<Value>(&closed).is_ok() {
                return Some(closed);
            }
        }
    }
    None
}

/// 清理悬挂分隔符并按栈补全闭合括号;返回 None 表示无法修复。
fn close_candidate(mut text: String, stack: &[JsonFrame]) -> Option<String> {
    loop {
        text = text.trim_end().to_string();
        if text.ends_with(',') {
            text.pop();
            continue;
        }
        if text.ends_with(':') {
            let bytes = text.as_bytes();
            let colon_index = bytes.len() - 1;
            let mut probe = colon_index;
            while probe > 0 && bytes[probe - 1].is_ascii_whitespace() {
                probe -= 1;
            }
            if probe == 0 || bytes[probe - 1] != b'"' {
                return None;
            }
            let closing_quote = probe - 1;
            let mut opening_quote = None;
            let mut scan = closing_quote;
            while scan > 0 {
                scan -= 1;
                if bytes[scan] == b'"' {
                    let mut backslashes = 0usize;
                    let mut back = scan;
                    while back > 0 && bytes[back - 1] == b'\\' {
                        backslashes += 1;
                        back -= 1;
                    }
                    if backslashes.is_multiple_of(2) {
                        opening_quote = Some(scan);
                        break;
                    }
                }
            }
            let opening_quote = opening_quote?;
            text.truncate(opening_quote);
            continue;
        }
        break;
    }
    for frame in stack.iter().rev() {
        text.push(match frame {
            JsonFrame::Object => '}',
            JsonFrame::Array => ']',
        });
    }
    Some(text)
}

/// TS parseJsonWithRepair:原文失败且修复文不同时重试修复文。
fn parse_json_with_repair(text: &str) -> Result<Value, String> {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Ok(value),
        Err(original) => {
            let repaired = repair_json(text);
            if repaired != text {
                serde_json::from_str::<Value>(&repaired).map_err(|error| error.to_string())
            } else {
                Err(original.to_string())
            }
        }
    }
}

/// 部分流式 JSON → 参数对象;非对象根(parse 失败/数组/标量)回退空 Map。
fn parse_streaming_json(partial: &str) -> Map<String, Value> {
    let trimmed = partial.trim();
    if trimmed.is_empty() {
        return Map::new();
    }
    let repaired = repair_json(trimmed);
    let closed_original = close_partial_json(trimmed);
    let closed_repaired = close_partial_json(&repaired);
    let direct = [trimmed, repaired.as_str()];
    for candidate in direct
        .into_iter()
        .chain(closed_original.iter().map(String::as_str))
        .chain(closed_repaired.iter().map(String::as_str))
    {
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(candidate) {
            return map;
        }
    }
    Map::new()
}

// ── 用量与停止原因 ───────────────────────────────────────────────────

/// TS calculateCost:每百万 token 费率,支持输入量分档;1h cache write 双倍输入价。
fn calculate_cost(model: &Model, usage: &mut Usage) {
    let input_tokens = usage.input + usage.cache_read + usage.cache_write;
    let mut rates = &model.cost.rates;
    let mut matched_threshold = -1i64;
    if let Some(tiers) = &model.cost.tiers {
        for tier in tiers {
            if input_tokens > tier.input_tokens_above && tier.input_tokens_above > matched_threshold
            {
                rates = &tier.rates;
                matched_threshold = tier.input_tokens_above;
            }
        }
    }
    let long_write = usage.cache_write_1h.unwrap_or(0);
    let short_write = usage.cache_write - long_write;
    let per_million = 1_000_000.0;
    usage.cost.input = rates.input / per_million * usage.input as f64;
    usage.cost.output = rates.output / per_million * usage.output as f64;
    usage.cost.cache_read = rates.cache_read / per_million * usage.cache_read as f64;
    usage.cost.cache_write = (rates.cache_write * short_write as f64
        + rates.input * 2.0 * long_write as f64)
        / per_million;
    usage.cost.total =
        usage.cost.input + usage.cost.output + usage.cost.cache_read + usage.cost.cache_write;
}

/// TS mapStopReason:未知 stop reason 视为流错误(蓝本 throw → catch 编码 error)。
fn map_stop_reason(
    reason: &str,
    stop_details: Option<&Value>,
) -> Result<(StopReason, Option<String>), String> {
    match reason {
        "end_turn" => Ok((StopReason::Stop, None)),
        "max_tokens" => Ok((StopReason::Length, None)),
        "tool_use" => Ok((StopReason::ToolUse, None)),
        "refusal" => {
            let explanation = stop_details
                .and_then(|details| details.get("explanation"))
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty());
            Ok((
                StopReason::Error,
                Some(
                    explanation
                        .unwrap_or("The model refused to complete the request")
                        .to_string(),
                ),
            ))
        }
        "pause_turn" | "stop_sequence" => Ok((StopReason::Stop, None)),
        other => Err(format!("Unhandled stop reason: {other}")),
    }
}

// ── SSE 解码(TS iterateSseMessages) ─────────────────────────────────

struct ServerSentEvent {
    event: Option<String>,
    data: String,
    raw: Vec<String>,
}

#[derive(Default)]
struct SseDecoder {
    event: Option<String>,
    data: Vec<String>,
    raw: Vec<String>,
    buffer: Vec<u8>,
}

impl SseDecoder {
    /// 喂入字节,返回其中完整的事件(支持 \r\n / \r / \n 与跨块断行、跨块 UTF-8)。
    fn push_bytes(&mut self, chunk: &[u8]) -> Vec<ServerSentEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((line_length, consumed)) = next_line(&self.buffer) {
            let line = String::from_utf8_lossy(&self.buffer[..line_length]).into_owned();
            self.buffer.drain(..consumed);
            if let Some(event) = self.decode_line(&line) {
                events.push(event);
            }
        }
        events
    }

    /// 流结束:处理残余字节(不足一行)并冲刷未终止事件。
    fn finish(&mut self) -> Vec<ServerSentEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&self.buffer).into_owned();
            self.buffer.clear();
            if let Some(event) = self.decode_line(&line) {
                events.push(event);
            }
        }
        if let Some(event) = self.flush() {
            events.push(event);
        }
        events
    }

    /// TS decodeSseLine:空行冲刷;":" 注释行忽略;event/data 字段累积。
    fn decode_line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            return self.flush();
        }
        self.raw.push(line.to_string());
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.find(':') {
            Some(colon_index) => (&line[..colon_index], &line[colon_index + 1..]),
            None => (line, ""),
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        None
    }

    fn flush(&mut self) -> Option<ServerSentEvent> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        let event = ServerSentEvent {
            event: self.event.take(),
            data: self.data.join("\n"),
            raw: std::mem::take(&mut self.raw),
        };
        self.data.clear();
        Some(event)
    }
}

/// 返回 (行内容长度, 含换行的消费长度)。
fn next_line(buffer: &[u8]) -> Option<(usize, usize)> {
    let carriage_return = buffer.iter().position(|&byte| byte == b'\r');
    let newline = buffer.iter().position(|&byte| byte == b'\n');
    let index = match (carriage_return, newline) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let mut consumed = index + 1;
    if buffer[index] == b'\r' && buffer.get(consumed) == Some(&b'\n') {
        consumed += 1;
    }
    Some((index, consumed))
}

// ── SSE 事件分类(TS iterateAnthropicEvents) ─────────────────────────

enum SseOutcome {
    Event(Value),
    Skip,
    Fatal(String),
}

fn handle_sse(
    sse: &ServerSentEvent,
    saw_message_start: &mut bool,
    saw_message_stop: &mut bool,
) -> SseOutcome {
    if sse.event.as_deref() == Some("error") {
        return SseOutcome::Fatal(sse.data.clone());
    }
    if !ANTHROPIC_MESSAGE_EVENTS.contains(&sse.event.as_deref().unwrap_or_default()) {
        return SseOutcome::Skip;
    }
    match parse_json_with_repair(&sse.data) {
        Ok(value) => {
            match value.get("type").and_then(Value::as_str) {
                Some("message_start") => *saw_message_start = true,
                Some("message_stop") => *saw_message_stop = true,
                _ => {}
            }
            SseOutcome::Event(value)
        }
        Err(message) => SseOutcome::Fatal(format!(
            "Could not parse Anthropic SSE event {}: {message}; data={}; raw={}",
            sse.event.as_deref().unwrap_or_default(),
            sse.data,
            sse.raw.join("\\n")
        )),
    }
}

// ── 事件聚合器(TS stream 的事件循环) ────────────────────────────────

/// Anthropic SSE 事件 → AssistantMessageEvent 的纯聚合逻辑(便于单测)。
struct AnthropicAggregator {
    model: Model,
    /// 请求 tools(OAuth 时 fromClaudeCodeName 的大小写还原需要)。
    tools: Vec<Tool>,
    is_oauth: bool,
    output: AssistantMessage,
    /// Anthropic 流块 index → output.content 下标。
    blocks: HashMap<i64, usize>,
    /// Anthropic 流块 index → 累积的 partial_json(仅 tool_use)。
    partial_json: HashMap<i64, String>,
}

impl AnthropicAggregator {
    fn new(model: &Model, tools: Vec<Tool>, is_oauth: bool) -> Self {
        Self {
            model: model.clone(),
            tools,
            is_oauth,
            output: new_assistant_message(model),
            blocks: HashMap::new(),
            partial_json: HashMap::new(),
        }
    }

    fn feed_sse(
        &mut self,
        sse: &ServerSentEvent,
        saw_message_start: &mut bool,
        saw_message_stop: &mut bool,
    ) -> Result<Vec<AssistantMessageEvent>, String> {
        match handle_sse(sse, saw_message_start, saw_message_stop) {
            SseOutcome::Skip => Ok(Vec::new()),
            SseOutcome::Fatal(message) => Err(message),
            SseOutcome::Event(value) => self.apply_event(&value),
        }
    }

    fn apply_event(&mut self, event: &Value) -> Result<Vec<AssistantMessageEvent>, String> {
        let mut events = Vec::new();
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => self.apply_message_start(event),
            Some("content_block_start") => self.apply_block_start(event, &mut events)?,
            Some("content_block_delta") => self.apply_block_delta(event, &mut events)?,
            Some("content_block_stop") => self.apply_block_stop(event, &mut events),
            Some("message_delta") => self.apply_message_delta(event)?,
            _ => {}
        }
        Ok(events)
    }

    fn apply_message_start(&mut self, event: &Value) {
        let message = &event["message"];
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            self.output.response_id = Some(id.to_string());
        }
        // TS 直接覆盖 output.model(响应模型即最终归属)
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            if !model.is_empty() {
                self.output.model = model.to_string();
            }
        }
        let usage = &message["usage"];
        let number_of = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0);
        let usage_out = &mut self.output.usage;
        usage_out.input = number_of("input_tokens");
        usage_out.output = number_of("output_tokens");
        usage_out.cache_read = number_of("cache_read_input_tokens");
        usage_out.cache_write = number_of("cache_creation_input_tokens");
        usage_out.cache_write_1h = Some(
            usage
                .get("cache_creation")
                .and_then(|creation| creation.get("ephemeral_1h_input_tokens"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
        );
        usage_out.total_tokens =
            usage_out.input + usage_out.output + usage_out.cache_read + usage_out.cache_write;
        calculate_cost(&self.model, &mut self.output.usage);
    }

    fn apply_block_start(
        &mut self,
        event: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Result<(), String> {
        let index = event.get("index").and_then(Value::as_i64).unwrap_or(-1);
        let block = &event["content_block"];
        match block.get("type").and_then(Value::as_str) {
            Some("fallback") => {
                if !self.output.content.is_empty() {
                    return Err(
                        "Anthropic performed an unsupported mid-output model fallback".to_string(),
                    );
                }
            }
            Some("text") => {
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.output.content.push(AssistantContent::Text {
                    text,
                    text_signature: None,
                });
                let position = self.output.content.len() - 1;
                self.blocks.insert(index, position);
                events.push(AssistantMessageEvent::TextStart {
                    content_index: position as u32,
                    partial: self.output.clone(),
                });
            }
            Some("thinking") => {
                let thinking = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let signature = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.output.content.push(AssistantContent::Thinking {
                    thinking,
                    thinking_signature: Some(signature),
                    redacted: false,
                });
                let position = self.output.content.len() - 1;
                self.blocks.insert(index, position);
                events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: position as u32,
                    partial: self.output.clone(),
                });
            }
            Some("redacted_thinking") => {
                let data = block
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.output.content.push(AssistantContent::Thinking {
                    thinking: "[Reasoning redacted]".to_string(),
                    thinking_signature: Some(data),
                    redacted: true,
                });
                let position = self.output.content.len() - 1;
                self.blocks.insert(index, position);
                events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: position as u32,
                    partial: self.output.clone(),
                });
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let raw_name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let name = if self.is_oauth {
                    from_claude_code_name(raw_name, &self.tools)
                } else {
                    raw_name.to_string()
                };
                let arguments = block
                    .get("input")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                self.output
                    .content
                    .push(AssistantContent::ToolCall(ToolCall {
                        id,
                        name,
                        arguments,
                        thought_signature: None,
                        namespace: None,
                    }));
                let position = self.output.content.len() - 1;
                self.blocks.insert(index, position);
                self.partial_json.insert(index, String::new());
                events.push(AssistantMessageEvent::ToolcallStart {
                    content_index: position as u32,
                    partial: self.output.clone(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_block_delta(
        &mut self,
        event: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Result<(), String> {
        let index = event.get("index").and_then(Value::as_i64).unwrap_or(-1);
        let delta = &event["delta"];
        match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => {
                let Some(&position) = self.blocks.get(&index) else {
                    return Ok(());
                };
                if matches!(
                    self.output.content.get(position),
                    Some(AssistantContent::Text { .. })
                ) {
                    let text = delta
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(AssistantContent::Text { text: target, .. }) =
                        self.output.content.get_mut(position)
                    {
                        target.push_str(text);
                    }
                    events.push(AssistantMessageEvent::TextDelta {
                        content_index: position as u32,
                        delta: text.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            Some("thinking_delta") => {
                let Some(&position) = self.blocks.get(&index) else {
                    return Ok(());
                };
                if matches!(
                    self.output.content.get(position),
                    Some(AssistantContent::Thinking { .. })
                ) {
                    let thinking = delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(AssistantContent::Thinking {
                        thinking: target, ..
                    }) = self.output.content.get_mut(position)
                    {
                        target.push_str(thinking);
                    }
                    events.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: position as u32,
                        delta: thinking.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            Some("input_json_delta") => {
                let Some(&position) = self.blocks.get(&index) else {
                    return Ok(());
                };
                if matches!(
                    self.output.content.get(position),
                    Some(AssistantContent::ToolCall(_))
                ) {
                    let piece = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let raw = self.partial_json.entry(index).or_default();
                    raw.push_str(piece);
                    let parsed = parse_streaming_json(raw);
                    if let Some(AssistantContent::ToolCall(tool_call)) =
                        self.output.content.get_mut(position)
                    {
                        tool_call.arguments = parsed;
                    }
                    events.push(AssistantMessageEvent::ToolcallDelta {
                        content_index: position as u32,
                        delta: piece.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            Some("signature_delta") => {
                let Some(&position) = self.blocks.get(&index) else {
                    return Ok(());
                };
                if let Some(AssistantContent::Thinking {
                    thinking_signature, ..
                }) = self.output.content.get_mut(position)
                {
                    let signature = delta
                        .get("signature")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let mut merged = thinking_signature.clone().unwrap_or_default();
                    merged.push_str(signature);
                    *thinking_signature = Some(merged);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_block_stop(&mut self, event: &Value, events: &mut Vec<AssistantMessageEvent>) {
        let index = event.get("index").and_then(Value::as_i64).unwrap_or(-1);
        let Some(position) = self.blocks.remove(&index) else {
            return;
        };
        match self.output.content[position].clone() {
            AssistantContent::Text { text, .. } => {
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: position as u32,
                    content: text,
                    partial: self.output.clone(),
                });
            }
            AssistantContent::Thinking { thinking, .. } => {
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: position as u32,
                    content: thinking,
                    partial: self.output.clone(),
                });
            }
            AssistantContent::ToolCall(mut tool_call) => {
                // partialJson 只是流式暂存;完成后以解析结果为最终 arguments
                let raw = self.partial_json.remove(&index).unwrap_or_default();
                tool_call.arguments = parse_streaming_json(&raw);
                self.output.content[position] = AssistantContent::ToolCall(tool_call.clone());
                events.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: position as u32,
                    tool_call,
                    partial: self.output.clone(),
                });
            }
        }
    }

    fn apply_message_delta(&mut self, event: &Value) -> Result<(), String> {
        let delta = &event["delta"];
        if let Some(reason) = delta
            .get("stop_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            self.output.raw_stop_reason = Some(reason.to_string());
            let stop_details = delta.get("stop_details").filter(|value| !value.is_null());
            let (stop_reason, error_message) = map_stop_reason(reason, stop_details)?;
            self.output.stop_reason = stop_reason;
            if let Some(message) = error_message {
                self.output.error_message = Some(message);
            }
        }
        // 只更新非 null 字段:保留 message_start 的 input_tokens(部分代理会缺省)
        if let Some(usage) = event.get("usage").filter(|usage| usage.is_object()) {
            if let Some(value) = usage.get("input_tokens").and_then(Value::as_i64) {
                self.output.usage.input = value;
            }
            if let Some(value) = usage.get("output_tokens").and_then(Value::as_i64) {
                self.output.usage.output = value;
            }
            if let Some(value) = usage.get("cache_read_input_tokens").and_then(Value::as_i64) {
                self.output.usage.cache_read = value;
            }
            if let Some(value) = usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_i64)
            {
                self.output.usage.cache_write = value;
            }
            // reasoning tokens 是 output 的子集
            if let Some(value) = usage
                .get("output_tokens_details")
                .and_then(|details| details.get("thinking_tokens"))
                .and_then(Value::as_i64)
            {
                self.output.usage.reasoning = Some(value);
            }
        }
        let usage = &mut self.output.usage;
        usage.total_tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
        calculate_cost(&self.model, &mut self.output.usage);
        Ok(())
    }

    /// 错误收尾:编码 stopReason + errorMessage(对齐 TS catch 分支)。
    fn error_final(
        mut self,
        reason: StopReason,
        message: String,
    ) -> (AssistantMessageEvent, AssistantMessage) {
        self.output.stop_reason = reason;
        self.output.error_message = Some(message);
        let event = AssistantMessageEvent::Error {
            reason,
            error: self.output.clone(),
        };
        (event, self.output)
    }

    /// TS stream 尾部检查:流正常结束后的终态判定;Some = 应编码为 error。
    fn tail_error(&self, aborted: bool) -> Option<String> {
        if aborted {
            return Some("Request was aborted".to_string());
        }
        if self.output.stop_reason == StopReason::Pending {
            return Some("Anthropic stream ended without a stop reason".to_string());
        }
        if self.output.stop_reason == StopReason::Aborted
            || self.output.stop_reason == StopReason::Error
        {
            return Some(
                self.output
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "An unknown error occurred".to_string()),
            );
        }
        None
    }

    fn done_final(self) -> (AssistantMessageEvent, AssistantMessage) {
        let reason = self.output.stop_reason;
        let event = AssistantMessageEvent::Done {
            reason,
            message: self.output.clone(),
        };
        (event, self.output)
    }
}

fn new_assistant_message(model: &Model) -> AssistantMessage {
    AssistantMessage {
        role: "assistant".to_string(),
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        usage: Usage::zero(),
        stop_reason: StopReason::Pending,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ts_nanos() / 1_000_000,
    }
}

// ── 请求体构造(TS buildParams,streamSimple 语义合并) ───────────────

/// 纯函数:构造 Anthropic Messages streaming 请求体(恒 `stream: true`)。
/// beta 特性走 `anthropic-beta` 请求头,不在体内(见 [`get_beta_features`])。
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> Value {
    let compat = get_anthropic_compat(model);
    let (_, cache_control) = get_cache_control(&compat, options);
    let resolved = resolve_stream_options(model, context, options);
    let is_oauth = options
        .and_then(|options| options.api_key.as_deref())
        .is_some_and(is_oauth_token);

    let transformed = transform_messages(&context.messages, model, normalize_tool_call_id);
    let normalize_tool_name = |name: &str| -> String {
        if is_oauth {
            to_claude_code_name(name)
        } else {
            name.to_string()
        }
    };
    let (mut immediate, mut deferred) = split_deferred_tools(
        &context.tools,
        &transformed,
        compat.supports_tool_references,
        &normalize_tool_name,
    );
    if immediate.is_empty() && !deferred.is_empty() {
        immediate = std::mem::take(&mut deferred);
    }
    let deferred_tool_names: HashSet<String> = deferred
        .iter()
        .map(|tool| normalize_tool_name(&tool.name))
        .collect();
    let converted = convert_messages(
        &transformed,
        is_oauth,
        cache_control.as_ref(),
        compat.allow_empty_signature,
        &deferred_tool_names,
        &normalize_tool_name,
    );

    let mut body = Map::new();
    body.insert("model".to_string(), json!(model.id));
    body.insert("messages".to_string(), Value::Array(converted));
    body.insert("max_tokens".to_string(), json!(resolved.max_tokens));
    body.insert("stream".to_string(), json!(true));

    // OAuth token 必须携带 Claude Code 身份 system 块
    if is_oauth {
        let mut system = vec![system_text_block(
            CLAUDE_CODE_IDENTITY_PROMPT,
            cache_control.as_ref(),
        )];
        if let Some(prompt) = &context.system_prompt {
            system.push(system_text_block(prompt, cache_control.as_ref()));
        }
        body.insert("system".to_string(), Value::Array(system));
    } else if let Some(prompt) = &context.system_prompt {
        body.insert(
            "system".to_string(),
            Value::Array(vec![system_text_block(prompt, cache_control.as_ref())]),
        );
    }

    // temperature 与 extended thinking 互斥
    if let Some(temperature) = resolved.temperature {
        if !resolved.thinking_enabled && compat.supports_temperature {
            body.insert("temperature".to_string(), json!(temperature));
        }
    }

    if !immediate.is_empty() || !deferred.is_empty() {
        let mut tools = convert_tools(
            &immediate,
            is_oauth,
            compat.supports_eager_tool_input_streaming,
            if compat.supports_cache_control_on_tools {
                cache_control.as_ref()
            } else {
                None
            },
            false,
        );
        tools.extend(convert_tools(
            &deferred,
            is_oauth,
            compat.supports_eager_tool_input_streaming,
            None,
            true,
        ));
        body.insert("tools".to_string(), Value::Array(tools));
    }

    if model.reasoning {
        if resolved.thinking_enabled {
            // budget-based thinking(旧模型);adaptive 路径待 compat 整合
            body.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": resolved.thinking_budget_tokens.unwrap_or(1024),
                    "display": "summarized",
                }),
            );
        } else if !matches!(map_level(model, "off"), MappedLevel::Null) {
            // thinkingLevelMap.off 显式 null = 禁止下发 disabled
            body.insert("thinking".to_string(), json!({ "type": "disabled" }));
        }
    }

    if let Some(user_id) = options
        .and_then(|options| options.metadata.as_ref())
        .and_then(|metadata| metadata.get("user_id"))
        .and_then(Value::as_str)
    {
        body.insert("metadata".to_string(), json!({ "user_id": user_id }));
    }

    if let Some(tool_choice) = options.and_then(|options| options.tool_choice) {
        let value = match tool_choice {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
        };
        body.insert("tool_choice".to_string(), json!({ "type": value }));
    }

    Value::Object(body)
}

// ── thinkingLevelMap 键查询(与 openai 移植同源) ─────────────────────

enum MappedLevel {
    Missing,
    Null,
    Value(String),
}

fn map_level(model: &Model, key: &str) -> MappedLevel {
    match model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(key))
    {
        None => MappedLevel::Missing,
        Some(None) => MappedLevel::Null,
        Some(Some(value)) => MappedLevel::Value(value.clone()),
    }
}

// ── 流式入口 ─────────────────────────────────────────────────────────

/// Anthropic Messages 流式生成:返回事件流(先 `start`,终止于 `done`/`error`)。
/// 失败/中止编码为 stopReason error/aborted 的最终消息,不 panic;
/// `signal` 取消即时生效(连接期与读取期)。
pub fn stream_anthropic_messages(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
    signal: Option<CancellationToken>,
) -> AssistantMessageEventStream {
    let (stream, writer) = event_stream::<AssistantMessageEvent, AssistantMessage>();
    tokio::spawn(run_stream(model, context, options, signal, writer));
    stream
}

async fn run_stream(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
    signal: Option<CancellationToken>,
    writer: EventStreamWriter<AssistantMessageEvent, AssistantMessage>,
) {
    let is_oauth = options
        .as_ref()
        .and_then(|options| options.api_key.as_deref())
        .is_some_and(is_oauth_token);
    let mut aggregator = AnthropicAggregator::new(&model, context.tools.clone(), is_oauth);
    writer.push(AssistantMessageEvent::Start {
        partial: aggregator.output.clone(),
    });

    if signal.as_ref().is_some_and(|token| token.is_cancelled()) {
        let (event, message) =
            aggregator.error_final(StopReason::Aborted, "Request was aborted".to_string());
        writer.push(event);
        writer.end(message);
        return;
    }

    // 请求体构造 + onPayload 观测/改写
    let mut body = build_request_body(&model, &context, options.as_ref());
    if let Some(on_payload) = options
        .as_ref()
        .and_then(|options| options.on_payload.as_ref())
    {
        if let Some(next) = on_payload(body.clone()).await {
            body = next;
        }
    }

    let max_retries = options
        .as_ref()
        .and_then(|options| options.max_retries)
        .unwrap_or(DEFAULT_MAX_RETRIES);
    let max_retry_delay_ms = options
        .as_ref()
        .and_then(|options| options.max_retry_delay_ms)
        .unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    let connect = async {
        let api_key = resolve_api_key(&model, options.as_ref()).map_err(|m| (m, false))?;
        let http = build_http_client(options.as_ref()).map_err(|m| (m, false))?;
        let compat = get_anthropic_compat(&model);
        let beta_features = get_beta_features(
            &model,
            &context,
            is_oauth,
            resolve_stream_options(&model, &context, options.as_ref()).thinking_enabled,
            options.as_ref(),
        );
        let retention = resolve_cache_retention(options.as_ref());
        let session_affinity =
            if retention != CacheRetention::None && compat.send_session_affinity_headers {
                options
                    .as_ref()
                    .and_then(|options| options.session_id.as_deref())
            } else {
                None
            };
        let headers = build_request_headers(
            &model,
            options.as_ref(),
            api_key.as_deref(),
            is_oauth,
            session_affinity,
            &beta_features,
        );
        let payload = serde_json::to_vec(&body)
            .map_err(|error| (format!("failed to serialize request body: {error}"), false))?;
        // TS retryProviderRequest:每次重试都是全新请求(重建 future 并重发)
        let mut retries_remaining = max_retries;
        loop {
            let outcome: Result<reqwest::Response, ProviderRequestError> = async {
                let response = http
                    .post(request_url(&model))
                    .headers(headers.clone())
                    .body(payload.clone())
                    .send()
                    .await
                    .map_err(|error| ProviderRequestError {
                        message: error.to_string(),
                        status: None,
                        headers: None,
                    })?;
                let status = response.status();
                if !status.is_success() {
                    // SDK 行为:非 2xx 抛带 status/headers 的 APIError,进入重试判定
                    let response_headers = response.headers().clone();
                    let text = response.text().await.unwrap_or_default();
                    return Err(ProviderRequestError {
                        message: format_http_error(status.as_u16(), &text),
                        status: Some(status.as_u16()),
                        headers: Some(response_headers),
                    });
                }
                if let Some(on_response) = options
                    .as_ref()
                    .and_then(|options| options.on_response.as_ref())
                {
                    on_response(&ProviderResponse {
                        status: status.as_u16(),
                        headers: headers_to_map(response.headers()),
                    });
                }
                Ok(response)
            }
            .await;
            match outcome {
                Ok(response) => {
                    return Ok::<reqwest::Response, (String, bool)>(response);
                }
                Err(error) => {
                    let retry_index = max_retries.saturating_sub(retries_remaining);
                    match retry_decision(&error, retry_index, retries_remaining, max_retry_delay_ms)
                    {
                        // 退避期间取消由外层 select! 感知(connect future 被 drop)
                        RetryDecision::Retry(delay_ms) => {
                            retries_remaining -= 1;
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        RetryDecision::DelayExceeded(message) => return Err((message, false)),
                        RetryDecision::NotRetryable => return Err((error.message, false)),
                    }
                }
            }
        }
    };
    let connected = if let Some(token) = &signal {
        tokio::select! {
            result = connect => result,
            _ = token.cancelled() => Err(("Request was aborted".to_string(), true)),
        }
    } else {
        connect.await
    };
    let mut response = match connected {
        Ok(response) => response,
        Err((message, aborted)) => {
            let reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            let (event, message) = aggregator.error_final(reason, message);
            writer.push(event);
            writer.end(message);
            return;
        }
    };

    let mut decoder = SseDecoder::default();
    let mut saw_message_start = false;
    let mut saw_message_stop = false;
    let mut aborted = false;
    let mut stream_error: Option<String> = None;
    let mut completed = false;
    'read: loop {
        let chunk = if let Some(token) = &signal {
            tokio::select! {
                chunk = response.chunk() => chunk,
                _ = token.cancelled() => {
                    aborted = true;
                    break 'read;
                }
            }
        } else {
            response.chunk().await
        };
        match chunk {
            Ok(Some(bytes)) => {
                for sse in decoder.push_bytes(&bytes) {
                    match aggregator.feed_sse(&sse, &mut saw_message_start, &mut saw_message_stop) {
                        Ok(events) => {
                            for event in events {
                                writer.push(event);
                            }
                        }
                        Err(message) => {
                            stream_error = Some(message);
                            break 'read;
                        }
                    }
                }
            }
            Ok(None) => {
                completed = true;
                break 'read;
            }
            Err(error) => {
                stream_error = Some(error.to_string());
                break 'read;
            }
        }
    }
    // 流正常结束:冲刷残余 SSE(对齐 TS 迭代器的收尾逻辑)
    if stream_error.is_none() && completed {
        for sse in decoder.finish() {
            match aggregator.feed_sse(&sse, &mut saw_message_start, &mut saw_message_stop) {
                Ok(events) => {
                    for event in events {
                        writer.push(event);
                    }
                }
                Err(message) => {
                    stream_error = Some(message);
                    break;
                }
            }
        }
    }
    // 中止时不做 message_stop 完整性检查(TS 生成器在读取时即抛 aborted)
    if stream_error.is_none() && !aborted && saw_message_start && !saw_message_stop {
        stream_error = Some("Anthropic stream ended before message_stop".to_string());
    }

    // TS 尾部检查 + catch:编码最终事件
    let cancelled = aborted || signal.as_ref().is_some_and(|token| token.is_cancelled());
    let error = stream_error.or_else(|| aggregator.tail_error(cancelled));
    let (terminal, message) = match error {
        Some(message) => {
            let reason = if cancelled {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            aggregator.error_final(reason, message)
        }
        None => aggregator.done_final(),
    };
    writer.push(terminal);
    writer.end(message);
}

// ── 单测 ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn anthropic_model(base_url: &str) -> Model {
        let mut model = Model::from_settings("claude-sonnet-4-5", base_url);
        model.api = "anthropic-messages".to_string();
        model.provider = "anthropic".to_string();
        model.max_tokens = 32_000;
        model
    }

    fn cross_model_model(base_url: &str) -> Model {
        let mut model = anthropic_model(base_url);
        model.provider = "proxy".to_string();
        model
    }

    fn user_message(text: &str) -> Message {
        Message::User(super::super::types::UserMessage {
            role: "user".to_string(),
            content: UserContent::text(text),
            timestamp: 0,
        })
    }

    fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantContent {
        AssistantContent::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::from_value(arguments).unwrap(),
            thought_signature: None,
            namespace: None,
        })
    }

    fn assistant_message(content: Vec<AssistantContent>, stop_reason: StopReason) -> Message {
        Message::Assistant(AssistantMessage {
            role: "assistant".to_string(),
            content,
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            response_model: None,
            response_id: None,
            usage: Usage::zero(),
            stop_reason,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        })
    }

    fn tool_result_message(tool_call_id: &str, tool_name: &str, text: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            role: "toolResult".to_string(),
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content: vec![TextOrImageContent::text(text)],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        })
    }

    fn context_of(messages: Vec<Message>) -> Context {
        Context {
            system_prompt: None,
            messages,
            tools: Vec::new(),
        }
    }

    fn tool(name: &str, schema: Value) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("{name} tool"),
            parameters: schema,
        }
    }

    // ── Claude Code 命名 ─────────────────────────────────────────────

    #[test]
    fn claude_code_naming_roundtrip() {
        assert_eq!(to_claude_code_name("read"), "Read");
        assert_eq!(to_claude_code_name("bash"), "Bash");
        assert_eq!(to_claude_code_name("my_tool"), "my_tool");
        let tools = vec![tool("get_weather", json!({}))];
        assert_eq!(from_claude_code_name("Get_Weather", &tools), "get_weather");
        assert_eq!(from_claude_code_name("unknown", &tools), "unknown");
        assert!(is_oauth_token("sk-ant-oat01-xxx"));
        assert!(!is_oauth_token("sk-ant-api03-xxx"));
    }

    #[test]
    fn default_supports_tool_references_matches_ts_rule() {
        let model = |id: &str| {
            let mut model = anthropic_model("https://api.anthropic.com");
            model.id = id.to_string();
            model
        };
        assert!(default_supports_tool_references(&model(
            "claude-sonnet-4-5-20250929"
        )));
        assert!(default_supports_tool_references(&model("claude-opus-4-6")));
        assert!(default_supports_tool_references(&model("claude-opus-5-x")));
        assert!(!default_supports_tool_references(&model(
            "claude-opus-4-1-20250805"
        )));
        assert!(!default_supports_tool_references(&model(
            "claude-sonnet-4-20250514"
        )));
        assert!(!default_supports_tool_references(&model(
            "claude-3-5-sonnet-20241022"
        )));
        assert!(!default_supports_tool_references(&model(
            "claude-haiku-4-5-20251001"
        )));
        let mut other = model("claude-sonnet-4-5");
        other.provider = "proxy".to_string();
        assert!(!default_supports_tool_references(&other));
    }

    // ── build_request_body ───────────────────────────────────────────

    #[test]
    fn basic_body_shape() {
        let model = anthropic_model("https://api.anthropic.com");
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());
        let body = build_request_body(&model, &context, None);

        assert_eq!(body["model"], "claude-sonnet-4-5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 32_000);
        assert_eq!(body["messages"][0]["role"], "user");
        // 末条 user 消息加 cache_control:字符串 content 被包装为块数组(TS 行为)
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }])
        );
        // 非 OAuth:system 为单 text 块并带默认 ephemeral cache_control
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 1);
        assert_eq!(system[0]["type"], "text");
        assert_eq!(system[0]["text"], "be brief");
        assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
        assert!(body["system"][0]["cache_control"].get("ttl").is_none());
        // 非 reasoning 模型不下发 thinking;无 tools 请求不带 tools
        assert!(body.get("thinking").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn thinking_budget_and_disabled() {
        let base_url = "https://api.anthropic.com";
        let mut model = anthropic_model(base_url);
        model.reasoning = true;
        let context = context_of(vec![user_message("hi")]);

        // reasoning=Medium → budget 8192(max_tokens 容纳 thinking 后钳制)
        let mut options = SimpleStreamOptions::default();
        options.reasoning = Some(ThinkingLevel::Medium);
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 8192);
        assert_eq!(body["thinking"]["display"], "summarized");
        assert_eq!(body["max_tokens"], 32_000);

        // 自定义预算 + 小模型上限:budget 被钳到 max_tokens - 1024
        let mut small = anthropic_model(base_url);
        small.max_tokens = 4096;
        small.reasoning = true;
        let mut custom = SimpleStreamOptions::default();
        custom.reasoning = Some(ThinkingLevel::High);
        let body = build_request_body(&small, &context, Some(&custom));
        assert_eq!(body["thinking"]["budget_tokens"], 3072);
        assert_eq!(body["max_tokens"], 4096);

        // reasoning=None → thinking disabled(thinkingLevelMap.off 未显式 null)
        let mut off = SimpleStreamOptions::default();
        off.reasoning = None;
        off.temperature = Some(0.7);
        let body = build_request_body(&model, &context, Some(&off));
        assert_eq!(body["thinking"], json!({ "type": "disabled" }));
        // off 显式 null → 不下发 disabled
        let mut keyed = anthropic_model(base_url);
        keyed.reasoning = true;
        keyed.thinking_level_map = Some(HashMap::from([("off".to_string(), None)]));
        let body = build_request_body(&keyed, &context, Some(&off));
        assert!(body.get("thinking").is_none());

        // thinking 开启时不带 temperature;关闭时带
        let mut with_temp = SimpleStreamOptions::default();
        with_temp.reasoning = Some(ThinkingLevel::Low);
        with_temp.temperature = Some(0.7);
        let body = build_request_body(&model, &context, Some(&with_temp));
        assert!(body.get("temperature").is_none());
        let body = build_request_body(&model, &context, Some(&off));
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn oauth_identity_headers_and_tool_naming() {
        let model = anthropic_model("https://api.anthropic.com");
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());
        context.tools = vec![
            tool("read", json!({ "type": "object" })),
            tool("my_tool", json!({})),
        ];
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("sk-ant-oat01-token".to_string());

        let body = build_request_body(&model, &context, Some(&options));
        // system:身份块 + 用户提示块,均带 cache_control
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 2);
        assert_eq!(system[0]["text"], CLAUDE_CODE_IDENTITY_PROMPT);
        assert_eq!(system[1]["text"], "be brief");
        assert_eq!(system[1]["cache_control"]["type"], "ephemeral");
        // 工具名归一为 CC 规范大小写
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools[0]["name"], "Read");
        assert_eq!(tools[0]["eager_input_streaming"], true);
        assert_eq!(tools[1]["name"], "my_tool");
        assert_eq!(tools[1]["input_schema"]["type"], "object");
        // 最后一个工具带 cache_control
        assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");
        // assistant 历史里的工具名同样归一
        context.messages = vec![
            user_message("run"),
            assistant_message(
                vec![tool_call("tc_1", "read", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result_message("tc_1", "read", "done"),
        ];
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][0]["name"], "Read");
    }

    #[test]
    fn tool_use_and_result_replay_with_id_normalization() {
        let model = cross_model_model("https://api.anthropic.com");
        let context = context_of(vec![
            user_message("weather?"),
            // 跨模型历史:provider 不同 → id 归一
            assistant_message(
                vec![tool_call(
                    "call_1|item|with|pipes",
                    "get_weather",
                    json!({"city": "Oslo"}),
                )],
                StopReason::ToolUse,
            ),
            tool_result_message("call_1|item|with|pipes", "get_weather", "18C"),
        ]);
        let body = build_request_body(&model, &context, None);
        let normalized = normalize_tool_call_id("call_1|item|with|pipes");
        assert_eq!(normalized, "call_1_item_with_pipes");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][0]["id"], normalized);
        assert_eq!(body["messages"][1]["content"][0]["input"]["city"], "Oslo");
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], normalized);
        assert_eq!(body["messages"][2]["content"][0]["content"], "18C");
        assert_eq!(body["messages"][2]["content"][0]["is_error"], false);
        // 末块(此 tool_result)带 cache_control
        assert_eq!(
            body["messages"][2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn thinking_replay_variants() {
        let model = anthropic_model("https://api.anthropic.com");
        let thinking = AssistantContent::Thinking {
            thinking: "hmm".to_string(),
            thinking_signature: Some("sig123".to_string()),
            redacted: false,
        };
        let redacted = AssistantContent::Thinking {
            thinking: "[Reasoning redacted]".to_string(),
            thinking_signature: Some("ENCRYPTED".to_string()),
            redacted: true,
        };
        let empty = AssistantContent::Thinking {
            thinking: "  ".to_string(),
            thinking_signature: None,
            redacted: false,
        };
        // 同模型:签名 thinking + redacted 保留;空 thinking 丢弃
        let context = context_of(vec![assistant_message(
            vec![thinking.clone(), redacted.clone(), empty],
            StopReason::Stop,
        )]);
        let body = build_request_body(&model, &context, None);
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "hmm");
        assert_eq!(blocks[0]["signature"], "sig123");
        assert_eq!(blocks[1]["type"], "redacted_thinking");
        assert_eq!(blocks[1]["data"], "ENCRYPTED");

        // 跨模型:redacted 丢弃、签名 thinking 转纯文本
        let cross = cross_model_model("https://api.anthropic.com");
        let body = build_request_body(
            &cross,
            &context_of(vec![assistant_message(
                vec![thinking, redacted],
                StopReason::Stop,
            )]),
            None,
        );
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "hmm");

        // 无签名的非空 thinking(中止残留)→ 转纯文本
        let unsigned = AssistantContent::Thinking {
            thinking: "partial".to_string(),
            thinking_signature: None,
            redacted: false,
        };
        let body = build_request_body(
            &model,
            &context_of(vec![assistant_message(vec![unsigned], StopReason::Stop)]),
            None,
        );
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "partial");
    }

    #[test]
    fn user_blocks_and_tool_result_images() {
        let mut model = anthropic_model("https://api.anthropic.com");
        model.input = vec![InputKind::Text, InputKind::Image];
        let context = context_of(vec![Message::User(super::super::types::UserMessage {
            role: "user".to_string(),
            content: UserContent::Blocks(vec![
                TextOrImageContent::Image {
                    data: "abc".to_string(),
                    mime_type: "image/png".to_string(),
                },
                TextOrImageContent::text("what is this?"),
            ]),
            timestamp: 0,
        })]);
        let body = build_request_body(&model, &context, None);
        let blocks = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "image");
        assert_eq!(blocks[0]["source"]["type"], "base64");
        assert_eq!(blocks[0]["source"]["media_type"], "image/png");
        assert_eq!(blocks[1]["text"], "what is this?");
        // 末块(文本)带 cache_control
        assert_eq!(blocks[1]["cache_control"]["type"], "ephemeral");

        // 纯图片 tool result → 补占位文本
        let context = context_of(vec![
            assistant_message(
                vec![tool_call("t1", "look", json!({}))],
                StopReason::ToolUse,
            ),
            Message::ToolResult(ToolResultMessage {
                role: "toolResult".to_string(),
                tool_call_id: "t1".to_string(),
                tool_name: "look".to_string(),
                content: vec![TextOrImageContent::Image {
                    data: "img".to_string(),
                    mime_type: "image/jpeg".to_string(),
                }],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 0,
            }),
        ]);
        let body = build_request_body(&model, &context, None);
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "tool_result");
        // 纯图片:占位文本 unshift 到首位,图片随后
        assert_eq!(content[0]["content"][0]["text"], IMAGE_ONLY_TEXT);
        assert_eq!(content[0]["content"][1]["type"], "image");
    }

    #[test]
    fn cache_retention_none_and_long_ttl() {
        let model = anthropic_model("https://api.anthropic.com");
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());

        let mut options = SimpleStreamOptions::default();
        options.cache_retention = Some(CacheRetention::None);
        let body = build_request_body(&model, &context, Some(&options));
        assert!(body["system"][0].get("cache_control").is_none());
        assert!(body["messages"][0]["content"].as_str().is_some()); // 字符串 content 不被包装

        let mut options = SimpleStreamOptions::default();
        options.cache_retention = Some(CacheRetention::Long);
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["system"][0]["cache_control"]["ttl"], "1h");

        // env 覆盖:PI_CACHE_RETENTION=long(无显式 retention)
        let mut options = SimpleStreamOptions::default();
        let mut env = HashMap::new();
        env.insert("PI_CACHE_RETENTION".to_string(), "long".to_string());
        options.env = Some(env);
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["ttl"],
            "1h"
        );
    }

    #[test]
    fn beta_features_and_metadata_and_tool_choice() {
        let model = anthropic_model("https://api.anthropic.com");
        let mut model = {
            let mut model = model;
            model.reasoning = true;
            model
        };
        let mut context = context_of(vec![user_message("hi")]);
        context.tools = vec![tool("read", json!({}))];

        // OAuth → claude-code + oauth beta;reasoning + thinking → interleaved
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("sk-ant-oat01-t".to_string());
        options.reasoning = Some(ThinkingLevel::Low);
        let features = get_beta_features(&model, &context, true, true, Some(&options));
        assert_eq!(
            features,
            vec![
                "claude-code-20250219".to_string(),
                "oauth-2025-04-20".to_string(),
                "interleaved-thinking-2025-05-14".to_string(),
            ]
        );
        // 非 thinking / 非 reasoning → 无 interleaved
        let features = get_beta_features(&model, &context, false, false, None);
        assert!(features.is_empty());
        model.reasoning = false;
        let features = get_beta_features(&model, &context, false, false, None);
        assert!(features.is_empty());

        // 显式 anthropic-beta 头优先(去重保序)
        model.headers = Some(HashMap::from([(
            "anthropic-beta".to_string(),
            "b2, b1 ,b2".to_string(),
        )]));
        let features = get_beta_features(&model, &context, true, true, None);
        assert_eq!(features, vec!["b2".to_string(), "b1".to_string()]);

        // metadata.user_id + tool_choice
        let mut options = SimpleStreamOptions::default();
        let mut metadata = HashMap::new();
        metadata.insert("user_id".to_string(), json!("user-42"));
        metadata.insert("ignored".to_string(), json!(1));
        options.metadata = Some(metadata);
        options.tool_choice = Some(ToolChoice::None);
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["metadata"]["user_id"], "user-42");
        assert!(body["metadata"].get("ignored").is_none());
        assert_eq!(body["tool_choice"]["type"], "none");
    }

    // ── SSE 解码 ─────────────────────────────────────────────────────

    fn sse_event_bytes(event: &str, data: &str) -> Vec<u8> {
        format!("event: {event}\ndata: {data}\n\n").into_bytes()
    }

    #[test]
    fn sse_decoder_handles_split_chunks_and_crlf() {
        let mut decoder = SseDecoder::default();
        let payload = format!(
            ": keep-alive comment\r\n\r\n{}{}{}",
            String::from_utf8(sse_event_bytes(
                "message_start",
                r#"{"type":"message_start"}"#
            ))
            .unwrap(),
            String::from_utf8(sse_event_bytes("content_block_delta", "{\"a\":\"你\"}")).unwrap(),
            String::from_utf8(sse_event_bytes(
                "message_stop",
                r#"{"type":"message_stop"}"#
            ))
            .unwrap(),
        );
        let bytes = payload.as_bytes();
        assert!(decoder.push_bytes(&bytes[..7]).is_empty());
        assert!(decoder.push_bytes(&bytes[7..40]).is_empty());
        let events = decoder.push_bytes(&bytes[40..]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, r#"{"type":"message_start"}"#);
        assert_eq!(events[1].data, "{\"a\":\"你\"}");
        assert_eq!(events[2].event.as_deref(), Some("message_stop"));

        // 多行 data 以 \n 连接;末尾无换行的事件由 finish 冲刷
        let mut decoder = SseDecoder::default();
        let events = decoder.push_bytes(b"event: x\ndata: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
        assert!(decoder.push_bytes(b"event: tail\ndata: {par").is_empty());
        let events = decoder.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("tail"));
        assert_eq!(events[0].data, "{par");
    }

    #[test]
    fn handle_sse_filters_and_reports_parse_errors() {
        let make = |event: Option<&str>, data: &str| ServerSentEvent {
            event: event.map(str::to_string),
            data: data.to_string(),
            raw: vec!["data: x".to_string()],
        };
        let mut saw_start = false;
        let mut saw_stop = false;
        // error 事件 → Fatal
        match handle_sse(&make(Some("error"), "boom"), &mut saw_start, &mut saw_stop) {
            SseOutcome::Fatal(message) => assert_eq!(message, "boom"),
            _ => panic!("expected fatal"),
        }
        // 未知 event → Skip
        assert!(matches!(
            handle_sse(&make(Some("ping"), "{}"), &mut saw_start, &mut saw_stop),
            SseOutcome::Skip
        ));
        // 无法解析的 data → Fatal 带上下文
        match handle_sse(
            &make(Some("message_start"), "not json"),
            &mut saw_start,
            &mut saw_stop,
        ) {
            SseOutcome::Fatal(message) => {
                assert!(message.contains("Could not parse Anthropic SSE event message_start"));
                assert!(message.contains("raw=data: x"));
            }
            _ => panic!("expected fatal"),
        }
        // 合法事件标记 saw 标志
        assert!(matches!(
            handle_sse(
                &make(Some("message_start"), r#"{"type":"message_start"}"#),
                &mut saw_start,
                &mut saw_stop
            ),
            SseOutcome::Event(_)
        ));
        assert!(saw_start && !saw_stop);
    }

    #[test]
    fn parse_json_with_repairs_control_characters() {
        // 裸控制字符 \u{1} 被修复为字面 \u0001 转义后解析回控制字符;合法 \n 转义保留
        let parsed = parse_json_with_repair("{\"a\": \"line\u{1}\\nbreak\"}").unwrap();
        assert_eq!(parsed["a"], "line\u{1}\nbreak");
        assert!(parse_json_with_repair("{").is_err());
    }

    // ── 聚合器 ───────────────────────────────────────────────────────

    fn aggregator() -> AnthropicAggregator {
        AnthropicAggregator::new(
            &anthropic_model("https://api.anthropic.com"),
            Vec::new(),
            false,
        )
    }

    fn apply(aggregator: &mut AnthropicAggregator, value: Value) -> Vec<AssistantMessageEvent> {
        aggregator.apply_event(&value).expect("event should apply")
    }

    #[test]
    fn aggregator_streams_text_and_usage() {
        let mut aggregate = aggregator();
        apply(
            &mut aggregate,
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_1",
                    "model": "claude-sonnet-4-5-20250929",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 1,
                        "cache_read_input_tokens": 40,
                        "cache_creation_input_tokens": 10,
                        "cache_creation": { "ephemeral_1h_input_tokens": 6 },
                    },
                },
            }),
        );
        assert_eq!(aggregate.output.response_id.as_deref(), Some("msg_1"));
        // 响应模型覆盖请求模型
        assert_eq!(aggregate.output.model, "claude-sonnet-4-5-20250929");
        assert_eq!(aggregate.output.usage.input, 100);
        assert_eq!(aggregate.output.usage.cache_read, 40);
        assert_eq!(aggregate.output.usage.cache_write, 10);
        assert_eq!(aggregate.output.usage.cache_write_1h, Some(6));
        assert_eq!(aggregate.output.usage.total_tokens, 151);

        let events = apply(
            &mut aggregate,
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextStart {
                content_index: 0,
                ..
            }
        ));

        let events = apply(
            &mut aggregate,
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hel" } }),
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextDelta { ref delta, .. } if delta == "Hel"
        ));
        apply(
            &mut aggregate,
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "lo" } }),
        );
        let events = apply(
            &mut aggregate,
            json!({ "type": "content_block_stop", "index": 0 }),
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { ref content, .. } if content == "Hello"
        ));

        // message_delta:usage 非空字段覆盖 + stop reason
        let events = apply(
            &mut aggregate,
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn" },
                "usage": { "output_tokens": 7 },
            }),
        );
        assert!(events.is_empty());
        assert_eq!(
            aggregate.output.raw_stop_reason.as_deref(),
            Some("end_turn")
        );
        assert_eq!(aggregate.output.stop_reason, StopReason::Stop);
        assert_eq!(aggregate.output.usage.output, 7);
        assert_eq!(
            aggregate.output.usage.input, 100,
            "input 保留 message_start 值"
        );
        // total = input + output + cacheRead + cacheWrite = 100+7+40+10
        assert_eq!(aggregate.output.usage.total_tokens, 157);
        assert!(aggregate.tail_error(false).is_none());

        let (terminal, message) = aggregate.done_final();
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                ..
            }
        ));
        assert_eq!(message.content[0], AssistantContent::text("Hello"));
    }

    #[test]
    fn aggregator_streams_thinking_with_signature_and_redacted() {
        let mut aggregate = aggregator();
        apply(
            &mut aggregate,
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }),
        );
        apply(
            &mut aggregate,
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "let " } }),
        );
        apply(
            &mut aggregate,
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig" } }),
        );
        let events = apply(
            &mut aggregate,
            json!({ "type": "content_block_stop", "index": 0 }),
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ThinkingEnd { .. }
        ));
        assert_eq!(
            aggregate.output.content[0],
            AssistantContent::Thinking {
                thinking: "let ".to_string(),
                thinking_signature: Some("sig".to_string()),
                redacted: false,
            }
        );

        apply(
            &mut aggregate,
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "redacted_thinking", "data": "XYZ" } }),
        );
        assert_eq!(
            aggregate.output.content[1],
            AssistantContent::Thinking {
                thinking: "[Reasoning redacted]".to_string(),
                thinking_signature: Some("XYZ".to_string()),
                redacted: true,
            }
        );
    }

    #[test]
    fn aggregator_streams_partial_tool_json() {
        let mut aggregate = aggregator();
        let start_events = apply(
            &mut aggregate,
            json!({
                "type": "content_block_start",
                "index": 2,
                "content_block": { "type": "tool_use", "id": "tc_1", "name": "get_weather", "input": {} },
            }),
        );
        assert!(matches!(
            start_events[0],
            AssistantMessageEvent::ToolcallStart {
                content_index: 0,
                ..
            }
        ));

        let events = apply(
            &mut aggregate,
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"city\": \"Os" } }),
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolcallDelta { ref delta, .. } if delta == "{\"city\": \"Os"
        ));
        // 部分 JSON:字符串值保留部分文本
        assert_eq!(
            aggregate.output.content[0],
            tool_call("tc_1", "get_weather", json!({"city": "Os"}))
        );

        apply(
            &mut aggregate,
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "lo\"}" } }),
        );
        let events = apply(
            &mut aggregate,
            json!({ "type": "content_block_stop", "index": 2 }),
        );
        let AssistantMessageEvent::ToolcallEnd { tool_call, .. } = &events[0] else {
            panic!("expected toolcall_end");
        };
        assert_eq!(
            tool_call.arguments.get("city").and_then(Value::as_str),
            Some("Oslo")
        );
        assert_eq!(aggregate.output.stop_reason, StopReason::Pending);
    }

    #[test]
    fn aggregator_maps_stop_reasons() {
        let cases = [
            ("end_turn", StopReason::Stop),
            ("max_tokens", StopReason::Length),
            ("tool_use", StopReason::ToolUse),
            ("pause_turn", StopReason::Stop),
            ("stop_sequence", StopReason::Stop),
        ];
        for (raw, expected) in cases {
            let mut aggregate = aggregator();
            apply(
                &mut aggregate,
                json!({ "type": "message_delta", "delta": { "stop_reason": raw } }),
            );
            assert_eq!(aggregate.output.stop_reason, expected, "raw={raw}");
        }

        // refusal → error + explanation
        let mut aggregate = aggregator();
        apply(
            &mut aggregate,
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "refusal", "stop_details": { "explanation": "nope" } },
            }),
        );
        assert_eq!(aggregate.output.stop_reason, StopReason::Error);
        assert_eq!(aggregate.output.error_message.as_deref(), Some("nope"));
        assert_eq!(
            aggregate.tail_error(false).as_deref(),
            Some("nope"),
            "error stop reason 编码为流错误"
        );

        // refusal 无 explanation → 默认文案
        let mut aggregate = aggregator();
        apply(
            &mut aggregate,
            json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" } }),
        );
        assert_eq!(
            aggregate.output.error_message.as_deref(),
            Some("The model refused to complete the request")
        );

        // 未知 stop reason → apply_event 报错(蓝本 throw)
        let mut aggregate = aggregator();
        let error = aggregate
            .apply_event(&json!({ "type": "message_delta", "delta": { "stop_reason": "mystery" } }))
            .unwrap_err();
        assert_eq!(error, "Unhandled stop reason: mystery");
    }

    #[test]
    fn aggregator_rejects_mid_output_fallback_and_tail_without_stop() {
        let mut aggregate = aggregator();
        apply(
            &mut aggregate,
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "fallback" } }),
        );
        assert!(aggregate.output.content.is_empty(), "首个 fallback 块忽略");
        // 先产出真实块,fallback 才算 mid-output 错误
        apply(
            &mut aggregate,
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
        );
        assert_eq!(aggregate.output.content.len(), 1);
        let error = aggregate
            .apply_event(&json!({
                "type": "content_block_start",
                "index": 2,
                "content_block": { "type": "fallback" },
            }))
            .unwrap_err();
        assert!(error.contains("mid-output model fallback"));

        // 流结束但没有 stop reason → tail error
        let aggregate = aggregator();
        assert_eq!(
            aggregate.tail_error(false).as_deref(),
            Some("Anthropic stream ended without a stop reason")
        );
        // 中止优先
        assert_eq!(
            aggregate.tail_error(true).as_deref(),
            Some("Request was aborted")
        );
    }

    #[test]
    fn oauth_tool_name_restored_from_context() {
        let tools = vec![tool("read", json!({}))];
        let mut aggregate =
            AnthropicAggregator::new(&anthropic_model("https://api.anthropic.com"), tools, true);
        apply(
            &mut aggregate,
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "tool_use", "id": "t", "name": "Read", "input": {} },
            }),
        );
        // Claude Code 规范名还原为上下文里的原名
        let AssistantContent::ToolCall(tool_call) = &aggregate.output.content[0] else {
            panic!("expected tool call");
        };
        assert_eq!(tool_call.name, "read");
    }

    // ── 流式入口(本机 HTTP) ─────────────────────────────────────────

    struct MockServer {
        base_url: String,
        head: std::sync::mpsc::Receiver<Vec<u8>>,
    }

    /// 启动单连接 mock:读完请求头(含 body)后写出 response 字节。
    fn spawn_mock_server(response: Vec<u8>) -> MockServer {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local addr");
        let (head_tx, head_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut total = read_until_headers_end(&stream);
            if let Some(length) = content_length(&total) {
                while total.len() < length {
                    let mut buffer = [0u8; 4096];
                    match std::io::Read::read(&mut &stream, &mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => total.extend_from_slice(&buffer[..n]),
                    }
                }
            }
            let _ = head_tx.send(total);
            let mut stream = stream;
            use std::io::Write;
            let _ = stream.write_all(&response);
            let _ = stream.flush();
            // 保持读端片刻,避免 response 未送达即复位
            std::thread::sleep(std::time::Duration::from_millis(200));
        });
        MockServer {
            base_url: format!("http://{address}"),
            head: head_rx,
        }
    }

    fn read_until_headers_end(mut stream: &std::net::TcpStream) -> Vec<u8> {
        use std::io::Read;
        let mut buffer = [0u8; 1024];
        let mut total = Vec::new();
        while !total.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => total.extend_from_slice(&buffer[..n]),
            }
        }
        total
    }

    fn content_length(head: &[u8]) -> Option<usize> {
        let text = String::from_utf8_lossy(head);
        text.lines()
            .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
            .and_then(|line| line.split(':').nth(1))
            .and_then(|value| value.trim().parse().ok())
    }

    fn sse_response(body: String) -> Vec<u8> {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn stream_completes_over_local_http() {
        let body = [
            sse_event_bytes(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_9","model":"claude-sonnet-4-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
            ),
            sse_event_bytes(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            sse_event_bytes(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            ),
            sse_event_bytes("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
            sse_event_bytes(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
            ),
            sse_event_bytes("message_stop", r#"{"type":"message_stop"}"#),
        ]
        .concat();
        let server = spawn_mock_server(sse_response(String::from_utf8(body).unwrap()));

        let model = anthropic_model(&server.base_url);
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("test-key".to_string());
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(options),
            None,
        );

        use futures::StreamExt;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        let terminal = events.last().expect("terminal event").clone();
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                ..
            }
        ));
        let AssistantMessageEvent::Done { message, .. } = terminal else {
            panic!("expected done");
        };
        assert_eq!(message.response_id.as_deref(), Some("msg_9"));
        assert_eq!(message.content, vec![AssistantContent::text("hi")]);
        assert_eq!(message.usage.input, 10);
        assert_eq!(message.usage.output, 2);
        assert_eq!(message.usage.total_tokens, 12);
        assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
        // message_start/message_delta 不产事件:Start + TextStart + TextDelta + TextEnd + Done
        assert_eq!(events.len(), 5);

        // 请求头与请求体校验
        let head = server
            .head
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let head_text = String::from_utf8_lossy(&head).to_ascii_lowercase();
        assert!(head_text.starts_with("post /v1/messages"));
        assert!(head_text.contains("x-api-key: test-key"));
        assert!(head_text.contains("anthropic-version: 2023-06-01"));
        assert!(head_text.contains("content-type: application/json"));
        let body_start = head
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap_or(head.len());
        let request_body: Value = serde_json::from_slice(&head[body_start..]).expect("json body");
        assert_eq!(request_body["stream"], true);
        assert_eq!(request_body["messages"][0]["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn stream_reports_http_error_as_error_event() {
        let server = spawn_mock_server(
            b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: 48\r\nConnection: close\r\n\r\n{\"type\":\"error\",\"error\":{\"message\":\"slow down\"}}".to_vec(),
        );
        let model = anthropic_model(&server.base_url);
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("k".to_string());
        // 429 默认可重试;显式关闭以断言错误文案透传
        options.max_retries = Some(0);
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(options),
            None,
        );
        use futures::StreamExt;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
        let AssistantMessageEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error event");
        };
        assert_eq!(*reason, StopReason::Error);
        assert_eq!(error.error_message.as_deref(), Some("429: slow down"));
        assert_eq!(error.stop_reason, StopReason::Error);
    }

    // ── provider 内层重试 ────────────────────────────────────────────

    fn provider_error(status: Option<u16>, headers: &[(&str, &str)]) -> ProviderRequestError {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                map.insert(name, value);
            }
        }
        ProviderRequestError {
            message: "429: slow down".to_string(),
            status,
            headers: Some(map),
        }
    }

    #[test]
    fn retry_decision_follows_sdk_policy() {
        let decide = |error: &ProviderRequestError, remaining: u32| {
            retry_decision(error, 0, remaining, DEFAULT_MAX_RETRY_DELAY_MS)
        };
        // x-should-retry 头优先于状态码
        assert!(matches!(
            decide(
                &provider_error(Some(429), &[("x-should-retry", "false")]),
                1
            ),
            RetryDecision::NotRetryable
        ));
        assert!(matches!(
            decide(&provider_error(Some(400), &[("x-should-retry", "true")]), 1),
            RetryDecision::Retry(_)
        ));
        // 状态码策略:408/409/429/5xx/无 status 可重试,其余不可
        for status in [408, 409, 429, 500, 503, 529] {
            assert!(
                matches!(
                    decide(&provider_error(Some(status), &[]), 1),
                    RetryDecision::Retry(_)
                ),
                "status {status} 应可重试"
            );
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(
                matches!(
                    decide(&provider_error(Some(status), &[]), 1),
                    RetryDecision::NotRetryable
                ),
                "status {status} 不应重试"
            );
        }
        assert!(matches!(
            decide(&provider_error(None, &[]), 1),
            RetryDecision::Retry(_)
        ));
        // 重试次数耗尽
        assert!(matches!(
            decide(&provider_error(Some(429), &[]), 0),
            RetryDecision::NotRetryable
        ));
    }

    #[test]
    fn retry_delay_uses_server_headers_and_cap() {
        let delay = |headers: &[(&str, &str)], max: u64| {
            get_retry_delay_ms(&provider_error(Some(429), headers), 0, max)
        };
        assert_eq!(delay(&[("retry-after-ms", "250")], 60_000), Ok(250));
        // TS parseFloat 宽松前缀
        assert_eq!(delay(&[("retry-after-ms", "120ms")], 60_000), Ok(120));
        assert_eq!(delay(&[("retry-after", "2")], 60_000), Ok(2000));
        assert_eq!(delay(&[("retry-after", "1.5")], 60_000), Ok(1500));
        // 超上限 → 立即失败并带上限文案
        assert_eq!(
            delay(&[("retry-after-ms", "70000")], 60_000),
            Err("Server requested 70s retry delay (max: 60s). 429: slow down".to_string())
        );
        // max_retry_delay_ms = 0 → 不限上限
        assert_eq!(delay(&[("retry-after-ms", "70000")], 0), Ok(70_000));
        // HTTP 日期:过去时刻钳到 0;未来时刻按差值生效
        assert_eq!(
            delay(&[("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT")], 60_000),
            Ok(0)
        );
        let future = (chrono::Utc::now() + chrono::Duration::seconds(30))
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let delay_ms = delay(&[("retry-after", future.as_str())], 60_000).expect("future date");
        assert!((29_000..=31_000).contains(&delay_ms), "got {delay_ms}");
    }

    #[test]
    fn retry_delay_exponential_backoff_bounds() {
        let delay_for = |retry_index: u32| {
            get_retry_delay_ms(&provider_error(Some(429), &[]), retry_index, 60_000)
                .expect("exponential delay")
        };
        // 首次 0.5s × [0.75, 1]
        let first = delay_for(0);
        assert!((375..=500).contains(&first), "got {first}");
        // 第 5 次:底数封顶 8s × [0.75, 1]
        let capped = delay_for(5);
        assert!((6000..=8000).contains(&capped), "got {capped}");
    }

    /// 多请求 mock:按顺序为每个响应接受一条新连接(Connection: close)。
    fn spawn_sequential_mock(
        responses: Vec<Vec<u8>>,
        served: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            for response in responses {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = read_until_headers_end(&stream);
                let mut stream = stream;
                use std::io::Write;
                let _ = stream.write_all(&response);
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn stream_retries_429_then_succeeds() {
        let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let retry_hint = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: text/plain\r\nRetry-After-Ms: 5\r\nConnection: close\r\n\r\ncalm down".to_vec();
        let success = sse_response(String::from_utf8(
            [
                sse_event_bytes(
                    "message_start",
                    r#"{"type":"message_start","message":{"id":"m2","model":"claude-sonnet-4-5","usage":{"input_tokens":3,"output_tokens":1}}}"#,
                ),
                sse_event_bytes(
                    "message_delta",
                    r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
                ),
                sse_event_bytes("message_stop", r#"{"type":"message_stop"}"#),
            ]
            .concat(),
        ).expect("utf8"));
        let base_url = spawn_sequential_mock(vec![retry_hint, success], served.clone());

        let model = anthropic_model(&base_url);
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("k".to_string()); // max_retries 用默认 2
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(options),
            None,
        );
        use futures::StreamExt;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        let AssistantMessageEvent::Done { message, .. } = events.last().unwrap() else {
            panic!("expected done after retry, got {:?}", events.last());
        };
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.usage.output, 4);
        assert_eq!(
            served.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "429 后应重发一次"
        );
    }

    #[tokio::test]
    async fn stream_respects_x_should_retry_false() {
        let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let rejected = b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nx-should-retry: false\r\nConnection: close\r\n\r\n{\"type\":\"error\",\"error\":{\"message\":\"nope\"}}".to_vec();
        let base_url = spawn_sequential_mock(vec![rejected], served.clone());

        let model = anthropic_model(&base_url);
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("k".to_string());
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(options),
            None,
        );
        use futures::StreamExt;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        let AssistantMessageEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error event");
        };
        assert_eq!(*reason, StopReason::Error);
        assert_eq!(error.error_message.as_deref(), Some("400: nope"));
        assert_eq!(
            served.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "x-should-retry: false 不应重试"
        );
    }

    #[tokio::test]
    async fn stream_cancel_interrupts_retry_backoff() {
        let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let slow = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: text/plain\r\nRetry-After-Ms: 10000\r\nConnection: close\r\n\r\nslow".to_vec();
        let base_url = spawn_sequential_mock(vec![slow], served.clone());

        let model = anthropic_model(&base_url);
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("k".to_string());
        let signal = CancellationToken::new();
        tokio::spawn({
            let signal = signal.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                signal.cancel();
            }
        });
        let started = std::time::Instant::now();
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(options),
            Some(signal),
        );
        use futures::StreamExt;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        let AssistantMessageEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error event");
        };
        assert_eq!(*reason, StopReason::Aborted);
        assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
        // 退避被取消打断,未等服务端要求的 10s
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(
            served.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "退避期间取消不应重发"
        );
    }

    #[tokio::test]
    async fn stream_without_api_key_errors_before_connect() {
        let model = anthropic_model("http://127.0.0.1:9");
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(SimpleStreamOptions::default()),
            None,
        );
        use futures::StreamExt;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        let AssistantMessageEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error event");
        };
        assert_eq!(*reason, StopReason::Error);
        assert_eq!(
            error.error_message.as_deref(),
            Some("No API key for provider: anthropic")
        );
    }

    #[tokio::test]
    async fn stream_cancel_mid_flight_yields_aborted() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let _ = read_until_headers_end(&stream);
            use std::io::Write;
            let mut stream = stream;
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.write_all(&sse_event_bytes(
                "message_start",
                r#"{"type":"message_start","message":{"id":"m","model":"claude-sonnet-4-5","usage":{}}}"#,
            ));
            let _ = stream.flush();
            // 不结束流,等待客户端取消
            std::thread::sleep(std::time::Duration::from_secs(30));
        });

        let model = anthropic_model(&format!("http://{address}"));
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("k".to_string());
        let signal = CancellationToken::new();
        let mut stream = stream_anthropic_messages(
            model,
            context_of(vec![user_message("hello")]),
            Some(options),
            Some(signal.clone()),
        );
        use futures::StreamExt;
        // 定时取消:服务端只发 message_start(不产事件)后保持连接
        tokio::spawn({
            let signal = signal.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                signal.cancel();
            }
        });
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        let AssistantMessageEvent::Error { reason, error } = events.last().unwrap() else {
            panic!("expected error event, got {:?}", events.last());
        };
        assert_eq!(*reason, StopReason::Aborted);
        assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
        // message_start 已消费:response_id 在中止消息上可见
        assert_eq!(error.response_id.as_deref(), Some("m"));
    }
}
