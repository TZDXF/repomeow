//! OpenAI Responses provider:pi-ai `api/openai-responses.ts` + `api/openai-responses-shared.ts`
//! (0.84.4)的 Rust 复刻。
//!
//! 组成:
//! - [`build_request_body`]:纯函数,把 [`Model`] + [`Context`] + [`SimpleStreamOptions`]
//!   序列化为 Responses `POST /responses` 请求体(input items、tools、reasoning/include、
//!   prompt cache 键与保留期)。
//! - [`stream_openai_responses`]:流式入口,直接用 reqwest 发请求并自行解析 SSE
//!   (async-openai 0.41 尚无 responses feature),把 Responses 流事件聚合为
//!   [`AssistantMessageEvent`] 推入 [`EventStreamWriter`];失败/中止编码进流
//!   (stopReason error/aborted + errorMessage),不 panic、不抛出。
//! - [`ResponsesAggregator`]:Responses SSE 事件 → 事件的纯聚合逻辑,便于单测。
//! - [`SseDecoder`]:字节流 → SSE data 载荷的纯解码器,便于单测。
//!
//! 与蓝本的已知偏差(对齐 openai_completions.rs 的既有取舍):
//! - 无 constrained sampling/grammar custom tools(grammarToolInputProperties 恒空,
//!   custom_tool_call 的 input 流式增量按原文拼接,不做 JSON 转义缓冲);
//! - 无 deferred tools(additional-tools / tool-search 两条路径不触发);
//! - 无 service tier 定价;HTTP 错误格式化保留状态码 + 截断后的响应体;
//! - Responses 专属 compat 尚未建模进 `Model.compat`,复用既有 `OpenAICompletionsCompat`
//!   的 supports_developer_role / supports_long_cache_retention / supports_strict_mode /
//!   supports_max_output_tokens 四个开关,其余按蓝本缺省行为实现。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::event_stream::{event_stream, EventStreamWriter};
use super::types::{
    user_agent, AssistantContent, AssistantMessage, AssistantMessageEvent,
    AssistantMessageEventStream, CacheRetention, Context, InputKind, Message, Model,
    SimpleStreamOptions, StopReason, TextOrImageContent, ThinkingLevel, Tool, ToolCall, ToolChoice,
    ToolResultMessage, Usage, UsageCost, UserContent,
};
use crate::time_util::now_ts_nanos;

const OPENAI_TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];
/// OpenAI Responses 拒绝低于 16 的 max_output_tokens。
const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: i64 = 16;
const TOOL_RESULT_IMAGE_TEXT: &str = "(see attached image)";
const TOOL_RESULT_EMPTY_TEXT: &str = "(no tool output)";
const TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: model does not support images)";
const USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const SYNTHETIC_TOOL_RESULT_TEXT: &str = "No result provided";
/// HTTP 错误响应体纳入错误文案的截断上限(对齐 pi MAX_PROVIDER_ERROR_BODY_CHARS)。
const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;
/// Provider 内层重试默认次数(对齐 OpenAI SDK 默认)。
const DEFAULT_PROVIDER_MAX_RETRIES: u32 = 2;
/// 服务端要求重试延迟的上限,超过即失败(对齐 TS DEFAULT_MAX_RETRY_DELAY_MS;0 = 不限制)。
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

// ── 上下文预算估算(TS utils/estimate.ts,与 openai_completions 同源) ──

const CHARS_PER_TOKEN: i64 = 4;
const ESTIMATED_IMAGE_CHARS: i64 = 4800;
const CONTEXT_SAFETY_TOKENS: i64 = 4096;
const MIN_MAX_TOKENS: i64 = 1;

// ── 鉴权与 compat ─────────────────────────────────────────────────────

/// TS getClientApiKey:显式 apiKey 优先;否则看 headers 是否自带鉴权(置 "unused"),
/// 都没有则报错(编码为流内 error 事件,不 panic)。
fn resolve_api_key(model: &Model, options: Option<&SimpleStreamOptions>) -> Result<String, String> {
    if let Some(api_key) = options.and_then(|options| options.api_key.as_ref()) {
        return Ok(api_key.clone());
    }
    let has_auth_header = |headers: Option<&HashMap<String, String>>| {
        headers.is_some_and(|headers| {
            headers.iter().any(|(key, value)| {
                (key.eq_ignore_ascii_case("authorization")
                    || key.eq_ignore_ascii_case("cf-aig-authorization"))
                    && !value.trim().is_empty()
            })
        })
    };
    if has_auth_header(model.headers.as_ref())
        || has_auth_header(options.and_then(|o| o.headers.as_ref()))
    {
        return Ok("unused".to_string());
    }
    Err(format!("No API key for provider: {}", model.provider))
}

fn push_custom_headers(
    source: Option<&HashMap<String, String>>,
    headers: &mut reqwest::header::HeaderMap,
) {
    let Some(source) = source else { return };
    for (key, value) in source {
        match (
            reqwest::header::HeaderName::from_bytes(key.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            (Ok(name), Ok(value)) => {
                headers.insert(name, value);
            }
            _ => eprintln!("[agent/llm] 忽略非法自定义 header: {key}"),
        }
    }
}

/// TS SessionAffinityFormat(openai = session_id + x-client-request-id;
/// openai-nosession = 仅 x-client-request-id;openrouter = x-session-id)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionAffinityFormat {
    Openai,
    OpenaiNoSession,
    Openrouter,
}

/// Responses 专属 compat 的解析结果。
#[derive(Clone, Copy, Debug)]
struct ResponsesCompat {
    supports_developer_role: bool,
    session_affinity: SessionAffinityFormat,
    supports_long_cache_retention: bool,
    supports_strict_mode: bool,
    supports_max_output_tokens: bool,
}

/// TS getCompat + detectSessionAffinityFormat。
fn get_compat(model: &Model) -> ResponsesCompat {
    let compat = model.compat.as_ref();
    let is_openrouter = model.provider == "openrouter" || model.base_url.contains("openrouter.ai");
    ResponsesCompat {
        supports_developer_role: compat
            .and_then(|compat| compat.supports_developer_role)
            .unwrap_or(true),
        session_affinity: if is_openrouter {
            SessionAffinityFormat::Openrouter
        } else {
            SessionAffinityFormat::Openai
        },
        supports_long_cache_retention: compat
            .and_then(|value| value.supports_long_cache_retention)
            .unwrap_or(true),
        // 蓝本 Responses 缺省 false;应用允许经 compat 开关显式打开。
        supports_strict_mode: compat
            .and_then(|compat| compat.supports_strict_mode)
            .unwrap_or(false),
        supports_max_output_tokens: compat
            .and_then(|value| value.supports_max_output_tokens)
            .unwrap_or(true),
    }
}

fn apply_session_affinity_headers(
    headers: &mut reqwest::header::HeaderMap,
    compat: &ResponsesCompat,
    session_id: &str,
) {
    let insert = |headers: &mut reqwest::header::HeaderMap, name: &str, value: &str| {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) {
            headers.insert(name, value);
        }
    };
    match compat.session_affinity {
        SessionAffinityFormat::Openrouter => insert(headers, "x-session-id", session_id),
        SessionAffinityFormat::Openai => {
            insert(headers, "session_id", session_id);
            insert(headers, "x-client-request-id", session_id);
        }
        SessionAffinityFormat::OpenaiNoSession => {
            insert(headers, "x-client-request-id", session_id)
        }
    }
}

/// TS formatProviderError:HTTP 错误保留状态码与截断后的响应体。
fn format_http_error(status: u16, body: &str) -> String {
    let trimmed = body.trim();
    let truncated = truncate_error_text(trimmed, MAX_PROVIDER_ERROR_BODY_CHARS);
    if truncated.is_empty() {
        format!("OpenAI API error ({status})")
    } else {
        format!("OpenAI API error ({status}): {truncated}")
    }
}

fn truncate_error_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let prefix: String = text.chars().take(max_chars).collect();
    let removed = text.chars().count() - max_chars;
    format!("{prefix}... [truncated {removed} chars]")
}

// ── 通用小工具 ────────────────────────────────────────────────────────

/// TS shortHash(32 位混淆哈希 ×2,base36 拼接),按 UTF-16 码元迭代。
fn short_hash(text: &str) -> String {
    let mut h1: u32 = 0xdeadbeef;
    let mut h2: u32 = 0x41c6ce57;
    for unit in text.encode_utf16() {
        h1 = (h1 ^ u32::from(unit)).wrapping_mul(2654435761);
        h2 = (h2 ^ u32::from(unit)).wrapping_mul(1597334677);
    }
    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2246822507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3266489909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2246822507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3266489909);
    format!("{}{}", to_base36(h2), to_base36(h1))
}

fn to_base36(mut value: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut buffer = Vec::new();
    while value > 0 {
        buffer.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    buffer.reverse();
    String::from_utf8(buffer).unwrap_or_default()
}

/// TS normalizeIdPart:非法字符消毒为 `_`、截 64 字符、去尾部下划线。
fn normalize_id_part(part: &str) -> String {
    let sanitized: String = part
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let normalized: String = sanitized.chars().take(64).collect();
    normalized.trim_end_matches('_').to_string()
}

/// TS buildForeignResponsesItemId:跨提供方 item id 重铸为 `fc_<hash>`。
fn build_foreign_responses_item_id(item_id: &str) -> String {
    let normalized = format!("fc_{}", short_hash(item_id));
    if normalized.chars().count() > 64 {
        normalized.chars().take(64).collect()
    } else {
        normalized
    }
}

/// TS clampOpenAIPromptCacheKey:超过 64 字符按 Unicode 字符截断。
fn clamp_openai_prompt_cache_key(key: &str) -> String {
    key.chars().take(64).collect()
}

/// TS encodeTextSignatureV1:文本回放签名 `{"v":1,"id":...[,"phase":...]}`。
fn encode_text_signature_v1(id: &str, phase: Option<&str>) -> String {
    let mut object = Map::new();
    object.insert("v".to_string(), json!(1));
    object.insert("id".to_string(), json!(id));
    if let Some(phase) = phase {
        object.insert("phase".to_string(), json!(phase));
    }
    Value::Object(object).to_string()
}

/// TS parseTextSignature:仅 `{"v":1,"id":...}` 形态按结构解析(phase 非法即丢弃),
/// 其余(含 v 缺失/非 1)整串视为 legacy id。
fn parse_text_signature(signature: &str) -> (String, Option<String>) {
    if signature.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(signature) {
            let is_v1 = parsed.get("v").and_then(Value::as_i64) == Some(1);
            if is_v1 {
                if let Some(id) = parsed.get("id").and_then(Value::as_str) {
                    let phase = parsed
                        .get("phase")
                        .and_then(Value::as_str)
                        .filter(|phase| matches!(*phase, "commentary" | "final_answer"))
                        .map(str::to_string);
                    return (id.to_string(), phase);
                }
            }
        }
    }
    (signature.to_string(), None)
}

// ── thinking 级别映射(TS clampThinkingLevel / thinkingLevelMap) ──────

fn level_key(level: crate::agent::llm::ModelThinkingLevel) -> &'static str {
    use crate::agent::llm::ModelThinkingLevel;
    match level {
        ModelThinkingLevel::Off => "off",
        ModelThinkingLevel::Minimal => "minimal",
        ModelThinkingLevel::Low => "low",
        ModelThinkingLevel::Medium => "medium",
        ModelThinkingLevel::High => "high",
        ModelThinkingLevel::Xhigh => "xhigh",
        ModelThinkingLevel::Max => "max",
    }
}

fn thinking_level_key(level: ThinkingLevel) -> &'static str {
    level_key(match level {
        ThinkingLevel::Minimal => crate::agent::llm::ModelThinkingLevel::Minimal,
        ThinkingLevel::Low => crate::agent::llm::ModelThinkingLevel::Low,
        ThinkingLevel::Medium => crate::agent::llm::ModelThinkingLevel::Medium,
        ThinkingLevel::High => crate::agent::llm::ModelThinkingLevel::High,
        ThinkingLevel::Xhigh => crate::agent::llm::ModelThinkingLevel::Xhigh,
        ThinkingLevel::Max => crate::agent::llm::ModelThinkingLevel::Max,
    })
}

/// thinkingLevelMap 键查询:Missing = 未配置,Null = 显式禁用。
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

/// `map[level] ?? level` 语义(null/缺键回退到级别名)。
fn map_level_or_key(model: &Model, key: &str) -> String {
    match map_level(model, key) {
        MappedLevel::Value(value) => value,
        MappedLevel::Missing | MappedLevel::Null => key.to_string(),
    }
}

fn supported_thinking_levels(model: &Model) -> Vec<crate::agent::llm::ModelThinkingLevel> {
    use crate::agent::llm::ModelThinkingLevel;
    if !model.reasoning {
        return vec![ModelThinkingLevel::Off];
    }
    const EXTENDED: [ModelThinkingLevel; 7] = [
        ModelThinkingLevel::Off,
        ModelThinkingLevel::Minimal,
        ModelThinkingLevel::Low,
        ModelThinkingLevel::Medium,
        ModelThinkingLevel::High,
        ModelThinkingLevel::Xhigh,
        ModelThinkingLevel::Max,
    ];
    EXTENDED
        .into_iter()
        .filter(|level| match map_level(model, level_key(*level)) {
            MappedLevel::Null => false,
            MappedLevel::Value(_) => true,
            MappedLevel::Missing => {
                !matches!(level, ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max)
            }
        })
        .collect()
}

/// TS clampThinkingLevel(SimpleStreamOptions.reasoning 恒非 off):就近回落,
/// 先向上再向下,全部不可用 → off → None。
fn clamp_thinking_level(model: &Model, level: ThinkingLevel) -> Option<ThinkingLevel> {
    use crate::agent::llm::ModelThinkingLevel;
    const EXTENDED: [ModelThinkingLevel; 7] = [
        ModelThinkingLevel::Off,
        ModelThinkingLevel::Minimal,
        ModelThinkingLevel::Low,
        ModelThinkingLevel::Medium,
        ModelThinkingLevel::High,
        ModelThinkingLevel::Xhigh,
        ModelThinkingLevel::Max,
    ];
    let available = supported_thinking_levels(model);
    let contains = |candidate: ModelThinkingLevel| available.contains(&candidate);
    let requested = EXTENDED
        .into_iter()
        .find(|candidate| level_key(*candidate) == thinking_level_key(level))?;
    if contains(requested) {
        return Some(level);
    }
    for candidate in EXTENDED.into_iter().skip(requested as usize) {
        match thinking_level_from_model(candidate) {
            Some(back) => {
                if contains(candidate) {
                    return Some(back);
                }
            }
            // Off:可用的只剩 off → 等价 reasoningEffort undefined
            None => return None,
        }
    }
    for candidate in EXTENDED.into_iter().take(requested as usize).rev() {
        match thinking_level_from_model(candidate) {
            Some(back) => {
                if contains(candidate) {
                    return Some(back);
                }
            }
            None => return None,
        }
    }
    None
}

fn thinking_level_from_model(
    level: crate::agent::llm::ModelThinkingLevel,
) -> Option<ThinkingLevel> {
    use crate::agent::llm::ModelThinkingLevel;
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
        ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
        ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
        ModelThinkingLevel::High => Some(ThinkingLevel::High),
        ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
        ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
    }
}

// ── 上下文预算估算 ────────────────────────────────────────────────────

fn ceil_div4(chars: i64) -> i64 {
    (chars + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
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

fn message_timestamp(message: &Message) -> i64 {
    match message {
        Message::User(user) => user.timestamp,
        Message::Assistant(assistant) => assistant.timestamp,
        Message::ToolResult(result) => result.timestamp,
    }
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

/// TS clampMaxTokensToContext(simple-options.ts)。
fn clamp_max_tokens_to_context(model: &Model, context: &Context, max_tokens: i64) -> i64 {
    if model.context_window <= 0 {
        return max_tokens.max(MIN_MAX_TOKENS);
    }
    let available = model.context_window - estimate_context_tokens(context) - CONTEXT_SAFETY_TOKENS;
    max_tokens.min(available.max(MIN_MAX_TOKENS))
}

fn insert_sampling_params(body: &mut Map<String, Value>, params: Option<&HashMap<String, Value>>) {
    let Some(params) = params else { return };
    // HashMap 无序:按 key 排序后应用,保证输出稳定(options 后写覆盖 model 先写)
    let mut keys: Vec<&String> = params.keys().collect();
    keys.sort();
    for key in keys {
        if let Some(value) = params.get(key) {
            body.insert(key.clone(), value.clone());
        }
    }
}

// ── 消息序列化(TS transformMessages + convertResponsesMessages) ─────

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

/// TS transformMessages:非图片模型降级图片;跨模型 replay 清理 thinking/签名与
/// tool call id(归一回调由调用方注入);孤儿 tool call 合成错误结果;
/// error/aborted assistant 整条丢弃。
fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_tool_call_id: Option<&dyn Fn(&str, &AssistantMessage) -> String>,
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
                        *blocks = replace_images_with_placeholder(blocks, USER_IMAGE_PLACEHOLDER);
                    }
                }
                first_pass.push(Message::User(user));
            }
            Message::ToolResult(result) => {
                let mut result = result.clone();
                if !supports_images {
                    result.content =
                        replace_images_with_placeholder(&result.content, TOOL_IMAGE_PLACEHOLDER);
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
                            // TS:签名以字符串真值判断("" 视为无签名)
                            let has_signature = thinking_signature
                                .as_ref()
                                .is_some_and(|signature| !signature.is_empty());
                            if redacted {
                                if is_same_model {
                                    content.push(AssistantContent::Thinking {
                                        thinking,
                                        thinking_signature,
                                        redacted,
                                    });
                                }
                            } else if is_same_model && has_signature {
                                content.push(AssistantContent::Thinking {
                                    thinking,
                                    thinking_signature,
                                    redacted,
                                });
                            } else if thinking.trim().is_empty() {
                                // 丢弃空 thinking
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
                                if let Some(normalize) = normalize_tool_call_id {
                                    let normalized = normalize(&tool_call.id, &assistant);
                                    if normalized != tool_call.id {
                                        tool_call_id_map
                                            .insert(tool_call.id.clone(), normalized.clone());
                                        tool_call.id = normalized;
                                    }
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

    // 第二遍:孤儿 tool call 补合成错误结果
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
                    // 错误/中止的回合不回放
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

/// TS convertResponsesMessages 的 normalizeToolCallId:openai 系 provider 保留
/// `{call_id}|{item_id}` 管道形状,item id 强制 `fc_` 前缀;跨提供方重铸 hash id。
fn normalize_responses_tool_call_id(id: &str, model: &Model, source: &AssistantMessage) -> String {
    if !OPENAI_TOOL_CALL_PROVIDERS.contains(&model.provider.as_str()) {
        return normalize_id_part(id);
    }
    let Some((call_id, item_id)) = id.split_once('|') else {
        return normalize_id_part(id);
    };
    let normalized_call_id = normalize_id_part(call_id);
    let is_foreign = source.provider != model.provider || source.api != model.api;
    let mut normalized_item_id = if is_foreign {
        build_foreign_responses_item_id(item_id)
    } else {
        normalize_id_part(item_id)
    };
    // OpenAI Responses API 要求 item id 以 "fc" 开头
    if !normalized_item_id.starts_with("fc_") {
        normalized_item_id = normalize_id_part(&format!("fc_{normalized_item_id}"));
    }
    format!("{normalized_call_id}|{normalized_item_id}")
}

/// TS convertToolResultOutput:模型支持图片时输出 input_text/input_image 数组,
/// 否则合并为纯文本(空文本回退占位符)。
fn convert_tool_result_output(model: &Model, content: &[TextOrImageContent]) -> Value {
    let text_result = content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text { text, .. } => Some(text.as_str()),
            TextOrImageContent::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let images: Vec<(&str, &str)> = content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Image { data, mime_type } => {
                Some((data.as_str(), mime_type.as_str()))
            }
            _ => None,
        })
        .collect();
    let has_text = !text_result.is_empty();

    if images.is_empty() || !model.input.contains(&InputKind::Image) {
        let text = if has_text {
            text_result
        } else if !images.is_empty() {
            TOOL_RESULT_IMAGE_TEXT.to_string()
        } else {
            TOOL_RESULT_EMPTY_TEXT.to_string()
        };
        return Value::String(text);
    }

    let mut output: Vec<Value> = Vec::new();
    if has_text {
        output.push(json!({ "type": "input_text", "text": text_result }));
    }
    for (data, mime_type) in images {
        output.push(json!({
            "type": "input_image",
            "detail": "auto",
            "image_url": format!("data:{mime_type};base64,{data}"),
        }));
    }
    Value::Array(output)
}

/// assistant 消息 → Responses 输出 item 序列(thinking 签名回放 / message /
/// function_call)。grammar custom tools 未建模,toolCall 一律回放为 function_call。
fn convert_assistant_output(
    assistant: &AssistantMessage,
    model: &Model,
    msg_index: usize,
) -> Vec<Value> {
    let is_same_provider_and_api =
        assistant.provider == model.provider && assistant.api == model.api;
    let is_same_model = is_same_provider_and_api && assistant.model == model.id;
    let is_different_model = is_same_provider_and_api && assistant.model != model.id;
    let mut text_block_index = 0usize;
    let mut output: Vec<Value> = Vec::new();

    for block in &assistant.content {
        match block {
            AssistantContent::Thinking {
                thinking_signature, ..
            } => {
                // 签名即完整 ResponseReasoningItem JSON,原样回放(解析失败静默跳过)
                if let Some(signature) = thinking_signature {
                    if let Ok(reasoning_item) = serde_json::from_str::<Value>(signature) {
                        output.push(reasoning_item);
                    }
                }
            }
            AssistantContent::Text {
                text,
                text_signature,
            } => {
                let (signature_id, signature_phase) = text_signature
                    .as_deref()
                    .map(parse_text_signature)
                    .unwrap_or_else(|| (String::new(), None));
                let has_signature_id = !signature_id.is_empty();
                let fallback_message_id = if text_block_index == 0 {
                    format!("msg_pi_{msg_index}")
                } else {
                    format!("msg_pi_{msg_index}_{text_block_index}")
                };
                text_block_index += 1;
                // OpenAI 要求 id 最长 64 字符
                let msg_id = if !has_signature_id {
                    fallback_message_id
                } else if signature_id.chars().count() > 64 {
                    format!("msg_{}", short_hash(&signature_id))
                } else {
                    signature_id
                };
                let mut item = json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text, "annotations": [] }],
                    "status": "completed",
                    "id": msg_id,
                });
                if let Some(phase) = signature_phase {
                    item["phase"] = json!(phase);
                }
                output.push(item);
            }
            AssistantContent::ToolCall(tool_call) => {
                let (call_id, item_id_raw) = match tool_call.id.split_once('|') {
                    Some((call_id, item_id)) => (call_id, Some(item_id)),
                    None => (tool_call.id.as_str(), None),
                };
                let starts_with_fc = item_id_raw.is_some_and(|id| id.starts_with("fc_"));
                // 不同模型回放丢 fc_ id(避免 OpenAI 的 fc↔rs 配对校验);
                // 同模型保留 fc_ id,非 fc_ id(如 ctc_ custom tool)一律丢弃。
                let drop_item_id = (is_different_model && starts_with_fc) || !starts_with_fc;
                let mut item = json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": tool_call.name,
                    "arguments": serde_json::to_string(&tool_call.arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                });
                if !drop_item_id {
                    if let Some(item_id) = item_id_raw {
                        item["id"] = json!(item_id);
                    }
                }
                if is_same_model {
                    if let Some(namespace) = &tool_call.namespace {
                        item["namespace"] = json!(namespace);
                    }
                }
                output.push(item);
            }
        }
    }
    output
}

/// TS convertResponsesMessages:消息历史 → Responses `input` items。
fn convert_responses_messages(
    model: &Model,
    context: &Context,
    compat: &ResponsesCompat,
) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    let normalize =
        |id: &str, source: &AssistantMessage| normalize_responses_tool_call_id(id, model, source);
    let transformed = transform_messages(&context.messages, model, Some(&normalize));

    if let Some(system_prompt) = &context.system_prompt {
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({ "role": role, "content": system_prompt }));
    }

    let mut msg_index = 0usize;
    for message in &transformed {
        match message {
            Message::User(user) => {
                let content: Vec<Value> = match &user.content {
                    UserContent::Text(text) => {
                        vec![json!({ "type": "input_text", "text": text })]
                    }
                    UserContent::Blocks(blocks) => blocks
                        .iter()
                        .map(|block| match block {
                            TextOrImageContent::Text { text, .. } => {
                                json!({ "type": "input_text", "text": text })
                            }
                            TextOrImageContent::Image { data, mime_type } => json!({
                                "type": "input_image",
                                "detail": "auto",
                                "image_url": format!("data:{mime_type};base64,{data}"),
                            }),
                        })
                        .collect(),
                };
                if content.is_empty() {
                    // TS:continue 跳过 msgIndex++
                    continue;
                }
                messages.push(json!({ "role": "user", "content": content }));
            }
            Message::Assistant(assistant) => {
                let output = convert_assistant_output(assistant, model, msg_index);
                if output.is_empty() {
                    // TS:continue 跳过 msgIndex++
                    continue;
                }
                messages.extend(output);
            }
            Message::ToolResult(result) => {
                let call_id = result
                    .tool_call_id
                    .split('|')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let output = convert_tool_result_output(model, &result.content);
                messages.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            }
        }
        msg_index += 1;
    }

    messages
}

// ── tools ─────────────────────────────────────────────────────────────

/// TS convertResponsesTools(function 形状,字段平铺;无 grammar 路径,
/// constrained sampling 未建模 → strict 恒 false,仅在支持时携带该键)。
fn convert_responses_tools(tools: &[Tool], supports_strict_mode: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut item = Map::new();
            item.insert("type".to_string(), json!("function"));
            item.insert("name".to_string(), json!(tool.name));
            item.insert("description".to_string(), json!(tool.description));
            item.insert("parameters".to_string(), tool.parameters.clone());
            if supports_strict_mode {
                item.insert("strict".to_string(), json!(false));
            }
            Value::Object(item)
        })
        .collect()
}

// ── 请求体构造 ────────────────────────────────────────────────────────

/// 纯函数:构造 OpenAI Responses streaming 请求体(恒 `stream: true`)。
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> Value {
    let compat = get_compat(model);
    let input = convert_responses_messages(model, context, &compat);

    let cache_retention = options
        .and_then(|options| options.cache_retention)
        .unwrap_or(CacheRetention::Short);
    let cache_session_id = if cache_retention == CacheRetention::None {
        None
    } else {
        options.and_then(|options| options.session_id.clone())
    };

    let mut body = Map::new();
    body.insert("model".to_string(), json!(model.id));
    body.insert("input".to_string(), Value::Array(input));
    body.insert("stream".to_string(), json!(true));
    if let Some(session_id) = &cache_session_id {
        body.insert(
            "prompt_cache_key".to_string(),
            json!(clamp_openai_prompt_cache_key(session_id)),
        );
    }
    if cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
        body.insert("prompt_cache_retention".to_string(), json!("24h"));
    }
    // supportsExplicitPromptCacheMode 缺省 false → prompt_cache_options 不下发
    body.insert("store".to_string(), json!(false));

    // max_output_tokens:simple 语义(options 缺省回退 model 上限,按上下文收敛),
    // 再抬到提供方最小值 16;上限未知(<= 0)时不下发(与 openai_completions 取舍一致)。
    if compat.supports_max_output_tokens {
        let requested = options
            .and_then(|options| options.max_tokens)
            .map(i64::from)
            .unwrap_or(model.max_tokens);
        if requested > 0 {
            let clamped = clamp_max_tokens_to_context(model, context, requested);
            if clamped > 0 {
                body.insert(
                    "max_output_tokens".to_string(),
                    json!(clamped.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
                );
            }
        }
    }

    if let Some(temperature) = options.and_then(|options| options.temperature) {
        body.insert("temperature".to_string(), json!(temperature));
    }

    let tools = convert_responses_tools(&context.tools, compat.supports_strict_mode);
    if !tools.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools));
    }

    if let Some(tool_choice) = options.and_then(|options| options.tool_choice) {
        let value = match tool_choice {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
        };
        body.insert("tool_choice".to_string(), json!(value));
    }

    // reasoning 参数(TS buildParams 的 reasoning 分支):
    // - 用户给了 reasoning 级别(且钳制后非 off)→ {effort, summary:"auto"} + include 密文;
    // - 未给(或钳到 off)→ 回退 thinkingLevelMap.off 映射,显式 null 则不下发;
    // - xai 恒 include reasoning.encrypted_content。
    if model.reasoning {
        let effort = options
            .and_then(|options| options.reasoning)
            .and_then(|level| clamp_thinking_level(model, level))
            .map(|level| map_level_or_key(model, thinking_level_key(level)));
        match effort {
            Some(effort) => {
                body.insert(
                    "reasoning".to_string(),
                    json!({ "effort": effort, "summary": "auto" }),
                );
                body.insert(
                    "include".to_string(),
                    json!(["reasoning.encrypted_content"]),
                );
            }
            None => {
                if model.provider != "github-copilot"
                    && !matches!(map_level(model, "off"), MappedLevel::Null)
                {
                    let value = match map_level(model, "off") {
                        MappedLevel::Value(value) => value,
                        _ => "none".to_string(),
                    };
                    body.insert("reasoning".to_string(), json!({ "effort": value }));
                }
            }
        }
        if model.provider == "xai" {
            body.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        }
    }

    // sampling_params:model 级先应用,options 级覆盖命名请求字段
    insert_sampling_params(&mut body, model.sampling_params.as_ref());
    insert_sampling_params(
        &mut body,
        options.and_then(|options| options.sampling_params.as_ref()),
    );

    Value::Object(body)
}

// ── cost(TS calculateCost,与 openai_completions 同源) ───────────────

/// 每百万 token 费率,支持输入量分档;1h cache write 双倍输入价。
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

// ── 部分容错 JSON(TS utils/json-parse.ts,与 openai_completions 同源) ─

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

#[derive(Clone, Copy)]
enum JsonFrame {
    Object,
    Array,
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

/// 部分流式 JSON → 参数对象;非对象根(parse 失败/数组/标量)回退空 Map。
/// 尝试顺序对齐 TS:原文 → 修复文 → 补全闭合(原文)→ 补全闭合(修复文)。
fn parse_streaming_json_object(partial: &str) -> Map<String, Value> {
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

// ── SSE 解码 ──────────────────────────────────────────────────────────

/// 字节流 → SSE data 载荷。规范要点:按 `\n` 分行(容忍 `\r\n`),
/// `data:` 行累积,空行分发事件(多行 data 以 `\n` 合并),
/// `event:`/`id:`/注释(`:` keep-alive)忽略——Responses 事件的类型在
/// data JSON 的 `type` 字段里。
struct SseDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseDecoder {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            data_lines: Vec::new(),
        }
    }

    /// 喂入一块字节,返回由此凑齐的事件 data 载荷(可能 0 个或多个)。
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut events = Vec::new();
        self.buffer.extend_from_slice(chunk);
        while let Some(position) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line_bytes: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(std::mem::take(&mut self.data_lines).join("\n"));
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data_lines
                    .push(data.strip_prefix(' ').unwrap_or(data).to_string());
            }
        }
        events
    }
}

// ── 流聚合器(TS processResponsesStream) ──────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotKind {
    Thinking,
    Text,
    ToolCall,
}

#[derive(Clone, Copy)]
struct Slot {
    kind: SlotKind,
    content_index: usize,
}

/// Responses SSE 事件 → AssistantMessageEvent 的纯聚合逻辑(便于单测)。
/// 块的 end 事件在 `response.output_item.done` 时就地发出(与 TS 一致),
/// 错误/中止路径不补发块 end。
struct ResponsesAggregator {
    model: Model,
    output: AssistantMessage,
    /// output_index → 已打开的输出槽
    slots: HashMap<i64, Slot>,
    /// reasoning item id → thinking 块下标(供终态回填 encrypted_content)
    reasoning_blocks_by_id: HashMap<String, usize>,
    /// function_call 的参数增量暂存(不进消息块,避免污染回放数据)
    tool_partial_json: HashMap<usize, String>,
    /// custom_tool_call 的 input 原文暂存
    tool_custom_input: HashMap<usize, String>,
    saw_terminal: bool,
}

impl ResponsesAggregator {
    fn new(model: &Model) -> Self {
        Self {
            model: model.clone(),
            output: new_assistant_message(model),
            slots: HashMap::new(),
            reasoning_blocks_by_id: HashMap::new(),
            tool_partial_json: HashMap::new(),
            tool_custom_input: HashMap::new(),
            saw_terminal: false,
        }
    }

    fn output(&self) -> &AssistantMessage {
        &self.output
    }

    fn get_slot(&self, output_index: i64, kind: SlotKind) -> Option<Slot> {
        let slot = self.slots.get(&output_index).copied()?;
        (slot.kind == kind).then_some(slot)
    }

    /// TS createSlot:按 item 类型打开新输出槽并推送对应 start 事件。
    fn create_slot(
        &mut self,
        output_index: i64,
        item: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Option<Slot> {
        let item_type = item.get("type").and_then(Value::as_str)?;
        match item_type {
            "reasoning" => {
                self.output.content.push(AssistantContent::Thinking {
                    thinking: String::new(),
                    thinking_signature: None,
                    redacted: false,
                });
                let content_index = self.output.content.len() - 1;
                let slot = Slot {
                    kind: SlotKind::Thinking,
                    content_index,
                };
                self.slots.insert(output_index, slot);
                events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: content_index as u32,
                    partial: self.output.clone(),
                });
                Some(slot)
            }
            "message" => {
                self.apply_message_phase_stop_reason(item);
                self.output.content.push(AssistantContent::text(""));
                let content_index = self.output.content.len() - 1;
                let slot = Slot {
                    kind: SlotKind::Text,
                    content_index,
                };
                self.slots.insert(output_index, slot);
                events.push(AssistantMessageEvent::TextStart {
                    content_index: content_index as u32,
                    partial: self.output.clone(),
                });
                Some(slot)
            }
            "function_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                let block = ToolCall {
                    id: format!("{call_id}|{item_id}"),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: Map::new(),
                    thought_signature: None,
                    namespace: item
                        .get("namespace")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                };
                self.output.content.push(AssistantContent::ToolCall(block));
                let content_index = self.output.content.len() - 1;
                let slot = Slot {
                    kind: SlotKind::ToolCall,
                    content_index,
                };
                self.slots.insert(output_index, slot);
                self.tool_partial_json.insert(
                    content_index,
                    item.get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                events.push(AssistantMessageEvent::ToolcallStart {
                    content_index: content_index as u32,
                    partial: self.output.clone(),
                });
                Some(slot)
            }
            "custom_tool_call" => {
                // grammar map 缺省 → input 属性名固定 "input"
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                let input = item
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut arguments = Map::new();
                arguments.insert("input".to_string(), json!(input));
                let block = ToolCall {
                    id: format!("{call_id}|{item_id}"),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments,
                    thought_signature: None,
                    namespace: item
                        .get("namespace")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                };
                self.output.content.push(AssistantContent::ToolCall(block));
                let content_index = self.output.content.len() - 1;
                let slot = Slot {
                    kind: SlotKind::ToolCall,
                    content_index,
                };
                self.slots.insert(output_index, slot);
                self.tool_custom_input
                    .insert(content_index, input.to_string());
                events.push(AssistantMessageEvent::ToolcallStart {
                    content_index: content_index as u32,
                    partial: self.output.clone(),
                });
                Some(slot)
            }
            _ => None,
        }
    }

    /// TS getOrCreateSlot:output_item.done 可能先于 added(或缺失 added)到达。
    fn slot_or_create(
        &mut self,
        output_index: i64,
        item: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Option<Slot> {
        if let Some(slot) = self.slots.get(&output_index).copied() {
            return Some(slot);
        }
        self.create_slot(output_index, item, events)
    }

    fn apply_message_phase_stop_reason(&mut self, item: &Value) {
        if item.get("type").and_then(Value::as_str) == Some("message")
            && item.get("phase").and_then(Value::as_str) == Some("final_answer")
        {
            self.output.stop_reason = StopReason::Stop;
        }
    }

    fn set_tool_arguments(&mut self, content_index: usize, arguments: Map<String, Value>) {
        if let Some(AssistantContent::ToolCall(tool_call)) =
            self.output.content.get_mut(content_index)
        {
            tool_call.arguments = arguments;
        }
    }

    fn set_tool_namespace_if_present(&mut self, content_index: usize, item: &Value) {
        if let Some(namespace) = item.get("namespace").and_then(Value::as_str) {
            if let Some(AssistantContent::ToolCall(tool_call)) =
                self.output.content.get_mut(content_index)
            {
                tool_call.namespace = Some(namespace.to_string());
            }
        }
    }

    /// 消费一个 Responses SSE 事件,返回由此产生的事件。
    /// Err = 需要中止流处理的错误(error 事件 / response.failed / 未知 status)。
    fn apply_event(&mut self, event: &Value) -> Result<Vec<AssistantMessageEvent>, String> {
        let mut events = Vec::new();
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            return Ok(events);
        };
        let output_index = event
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);

        match event_type {
            "response.created" => {
                if let Some(id) = event.pointer("/response/id").and_then(Value::as_str) {
                    self.output.response_id = Some(id.to_string());
                }
            }
            "response.output_item.added" => {
                if let Some(item) = event.get("item") {
                    self.create_slot(output_index, item, &mut events);
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::Thinking) else {
                    return Ok(events);
                };
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    if let Some(AssistantContent::Thinking { thinking, .. }) =
                        self.output.content.get_mut(slot.content_index)
                    {
                        thinking.push_str(delta);
                    }
                    events.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: slot.content_index as u32,
                        delta: delta.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.reasoning_summary_part.done" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::Thinking) else {
                    return Ok(events);
                };
                if let Some(AssistantContent::Thinking { thinking, .. }) =
                    self.output.content.get_mut(slot.content_index)
                {
                    thinking.push_str("\n\n");
                }
                events.push(AssistantMessageEvent::ThinkingDelta {
                    content_index: slot.content_index as u32,
                    delta: "\n\n".to_string(),
                    partial: self.output.clone(),
                });
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::Text) else {
                    return Ok(events);
                };
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    if let Some(AssistantContent::Text { text, .. }) =
                        self.output.content.get_mut(slot.content_index)
                    {
                        text.push_str(delta);
                    }
                    events.push(AssistantMessageEvent::TextDelta {
                        content_index: slot.content_index as u32,
                        delta: delta.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::ToolCall) else {
                    return Ok(events);
                };
                if !self.tool_partial_json.contains_key(&slot.content_index) {
                    return Ok(events);
                }
                let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                    return Ok(events);
                };
                let parsed = {
                    let entry = self
                        .tool_partial_json
                        .entry(slot.content_index)
                        .or_default();
                    entry.push_str(delta);
                    parse_streaming_json_object(entry)
                };
                self.set_tool_arguments(slot.content_index, parsed);
                events.push(AssistantMessageEvent::ToolcallDelta {
                    content_index: slot.content_index as u32,
                    delta: delta.to_string(),
                    partial: self.output.clone(),
                });
            }
            "response.function_call_arguments.done" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::ToolCall) else {
                    return Ok(events);
                };
                let Some(previous) = self.tool_partial_json.get(&slot.content_index) else {
                    return Ok(events);
                };
                let previous = previous.clone();
                let arguments_text = event
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.tool_partial_json
                    .insert(slot.content_index, arguments_text.clone());
                let parsed = parse_streaming_json_object(&arguments_text);
                self.set_tool_arguments(slot.content_index, parsed);
                // 最终参数是已累计前缀的延续时,补发剩余增量
                if arguments_text.starts_with(previous.as_str()) {
                    let delta = &arguments_text[previous.len()..];
                    if !delta.is_empty() {
                        events.push(AssistantMessageEvent::ToolcallDelta {
                            content_index: slot.content_index as u32,
                            delta: delta.to_string(),
                            partial: self.output.clone(),
                        });
                    }
                }
            }
            "response.custom_tool_call_input.delta" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::ToolCall) else {
                    return Ok(events);
                };
                let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                    return Ok(events);
                };
                if let Some(buffer) = self.tool_custom_input.get_mut(&slot.content_index) {
                    buffer.push_str(delta);
                    let mut arguments = Map::new();
                    arguments.insert("input".to_string(), json!(buffer.as_str()));
                    self.set_tool_arguments(slot.content_index, arguments);
                    // 偏差:无 grammar 缓冲,增量按原文透传(不做 JSON 字符串转义)
                    events.push(AssistantMessageEvent::ToolcallDelta {
                        content_index: slot.content_index as u32,
                        delta: delta.to_string(),
                        partial: self.output.clone(),
                    });
                }
            }
            "response.custom_tool_call_input.done" => {
                let Some(slot) = self.get_slot(output_index, SlotKind::ToolCall) else {
                    return Ok(events);
                };
                let input = event
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(buffer) = self.tool_custom_input.get_mut(&slot.content_index) {
                    let delta_text = if input.starts_with(buffer.as_str()) {
                        input[buffer.len()..].to_string()
                    } else {
                        input.clone()
                    };
                    *buffer = input;
                    let mut arguments = Map::new();
                    arguments.insert("input".to_string(), json!(buffer.as_str()));
                    self.set_tool_arguments(slot.content_index, arguments);
                    if !delta_text.is_empty() {
                        events.push(AssistantMessageEvent::ToolcallDelta {
                            content_index: slot.content_index as u32,
                            delta: delta_text,
                            partial: self.output.clone(),
                        });
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    self.apply_message_phase_stop_reason(item);
                    self.finish_output_item(output_index, item, &mut events);
                }
            }
            "response.completed" | "response.incomplete" => {
                let absent = Value::Null;
                self.finalize_response(event.get("response").unwrap_or(&absent))?;
            }
            "response.failed" => {
                self.saw_terminal = true;
                let response = event.get("response");
                if let Some(status) = response
                    .and_then(|response| response.get("status"))
                    .and_then(Value::as_str)
                {
                    self.output.raw_stop_reason = Some(status.to_string());
                }
                let error = response
                    .and_then(|response| response.get("error"))
                    .filter(|error| !error.is_null());
                let details = response
                    .and_then(|response| response.get("incomplete_details"))
                    .filter(|details| !details.is_null());
                let message = if let Some(error) = error {
                    let code = error
                        .get("code")
                        .and_then(Value::as_str)
                        .filter(|code| !code.is_empty())
                        .unwrap_or("unknown");
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .filter(|message| !message.is_empty())
                        .unwrap_or("no message");
                    format!("{code}: {message}")
                } else if let Some(reason) = details
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                    .filter(|reason| !reason.is_empty())
                {
                    format!("incomplete: {reason}")
                } else {
                    "Unknown error (no error details in response)".to_string()
                };
                return Err(message);
            }
            "error" => {
                let code = event
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("undefined");
                let message = event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("undefined");
                return Err(format!("Error Code {code}: {message}"));
            }
            _ => {}
        }
        Ok(events)
    }

    /// TS output_item.done 分支:按 item 类型收尾对应槽(缺失时补建)。
    fn finish_output_item(
        &mut self,
        output_index: i64,
        item: &Value,
        events: &mut Vec<AssistantMessageEvent>,
    ) {
        let Some(slot) = self.slot_or_create(output_index, item, events) else {
            return;
        };
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        match (item_type, slot.kind) {
            ("reasoning", SlotKind::Thinking) => {
                let join_text = |parts: Option<&Value>| -> String {
                    parts
                        .and_then(Value::as_array)
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .unwrap_or_default()
                };
                let summary_text = join_text(item.get("summary"));
                let content_text = join_text(item.get("content"));
                let current_thinking = match self.output.content.get(slot.content_index) {
                    Some(AssistantContent::Thinking { thinking, .. }) => thinking.clone(),
                    _ => String::new(),
                };
                let final_text = if !summary_text.is_empty() {
                    summary_text
                } else if !content_text.is_empty() {
                    content_text
                } else {
                    current_thinking
                };
                if let Some(AssistantContent::Thinking {
                    thinking,
                    thinking_signature,
                    ..
                }) = self.output.content.get_mut(slot.content_index)
                {
                    *thinking = final_text.clone();
                    // 签名 = 完整 reasoning item JSON(store:false 多轮回放依赖)
                    *thinking_signature = Some(item.to_string());
                }
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    self.reasoning_blocks_by_id
                        .insert(id.to_string(), slot.content_index);
                }
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: slot.content_index as u32,
                    content: final_text,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            ("message", SlotKind::Text) => {
                let text = item
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|part| {
                                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                    part.get("text").and_then(Value::as_str).unwrap_or_default()
                                } else {
                                    part.get("refusal")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                }
                            })
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                let phase = item.get("phase").and_then(Value::as_str);
                if let Some(AssistantContent::Text {
                    text: block_text,
                    text_signature,
                }) = self.output.content.get_mut(slot.content_index)
                {
                    *block_text = text.clone();
                    *text_signature = Some(encode_text_signature_v1(item_id, phase));
                }
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: slot.content_index as u32,
                    content: text,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            ("function_call", SlotKind::ToolCall)
                if self.tool_partial_json.contains_key(&slot.content_index) =>
            {
                let partial = self
                    .tool_partial_json
                    .get(&slot.content_index)
                    .cloned()
                    .unwrap_or_default();
                let item_arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // TS:item.arguments 为空串时回退已累计 partialJson
                let source = if item_arguments.is_empty() {
                    if partial.is_empty() {
                        "{}"
                    } else {
                        partial.as_str()
                    }
                } else {
                    item_arguments
                };
                let parsed = parse_streaming_json_object(source);
                self.set_tool_arguments(slot.content_index, parsed);
                self.set_tool_namespace_if_present(slot.content_index, item);
                self.tool_partial_json.remove(&slot.content_index);
                let tool_call = match self.output.content.get(slot.content_index) {
                    Some(AssistantContent::ToolCall(tool_call)) => tool_call.clone(),
                    _ => unreachable!("tool call slot content index invariant"),
                };
                events.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: slot.content_index as u32,
                    tool_call,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            ("custom_tool_call", SlotKind::ToolCall)
                if self.tool_custom_input.contains_key(&slot.content_index) =>
            {
                let buffered = self
                    .tool_custom_input
                    .get(&slot.content_index)
                    .cloned()
                    .unwrap_or_default();
                let input = item
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or(&buffered)
                    .to_string();
                let mut arguments = Map::new();
                arguments.insert("input".to_string(), json!(input));
                self.set_tool_arguments(slot.content_index, arguments);
                self.set_tool_namespace_if_present(slot.content_index, item);
                self.tool_custom_input.remove(&slot.content_index);
                let tool_call = match self.output.content.get(slot.content_index) {
                    Some(AssistantContent::ToolCall(tool_call)) => tool_call.clone(),
                    _ => unreachable!("tool call slot content index invariant"),
                };
                events.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: slot.content_index as u32,
                    tool_call,
                    partial: self.output.clone(),
                });
                self.slots.remove(&output_index);
            }
            _ => {}
        }
    }

    /// Azure 等网关可能只在终态 response.output 里给 encrypted_content:
    /// 对已落签名的 thinking 块回填该字段。
    fn backfill_reasoning_signatures(&mut self, response_output: &[Value]) {
        for item in response_output {
            if item.get("type").and_then(Value::as_str) != Some("reasoning") {
                continue;
            }
            let Some(encrypted_content) = item.get("encrypted_content").and_then(Value::as_str)
            else {
                continue;
            };
            let Some(item_id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(&content_index) = self.reasoning_blocks_by_id.get(item_id) else {
                continue;
            };
            let signature =
                match self
                    .output
                    .content
                    .get(content_index)
                    .and_then(|block| match block {
                        AssistantContent::Thinking {
                            thinking_signature, ..
                        } => thinking_signature.clone(),
                        _ => None,
                    }) {
                    Some(signature) => signature,
                    None => continue,
                };
            let Ok(mut stored) = serde_json::from_str::<Value>(&signature) else {
                continue;
            };
            if stored
                .get("encrypted_content")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
            {
                continue;
            }
            if let Some(stored_object) = stored.as_object_mut() {
                stored_object.insert("encrypted_content".to_string(), json!(encrypted_content));
            }
            if let Some(AssistantContent::Thinking {
                thinking_signature, ..
            }) = self.output.content.get_mut(content_index)
            {
                *thinking_signature = Some(stored.to_string());
            }
        }
    }

    /// TS finalizeResponse:回填签名、记账 usage/cost、映射停止原因。
    fn finalize_response(&mut self, response: &Value) -> Result<(), String> {
        self.saw_terminal = true;

        if let Some(output_items) = response.get("output").and_then(Value::as_array) {
            self.backfill_reasoning_signatures(output_items);
        }
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.output.response_id = Some(id.to_string());
        }

        if let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) {
            let number_of =
                |key: &str| usage.get(key).and_then(Value::as_f64).unwrap_or(0.0) as i64;
            let input_details = usage.get("input_tokens_details");
            // OpenAI 把缓存命中/写入计入 input_tokens,记账时扣除
            let cached_tokens = input_details
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let cache_write_tokens = input_details
                .and_then(|details| details.get("cache_write_tokens"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let mut parsed = Usage {
                input: (number_of("input_tokens") - cached_tokens - cache_write_tokens).max(0),
                output: number_of("output_tokens"),
                cache_read: cached_tokens,
                cache_write: cache_write_tokens,
                cache_write_1h: None,
                reasoning: usage
                    .pointer("/output_tokens_details/reasoning_tokens")
                    .and_then(Value::as_i64),
                total_tokens: number_of("total_tokens"),
                cost: UsageCost::default(),
            };
            calculate_cost(&self.model, &mut parsed);
            self.output.usage = parsed;
        } else {
            let mut zero = Usage::zero();
            calculate_cost(&self.model, &mut zero);
            self.output.usage = zero;
        }

        let status = response.get("status").and_then(Value::as_str);
        let incomplete_reason = response
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str);
        self.output.raw_stop_reason = match incomplete_reason {
            Some(reason) => Some(format!("{}.{reason}", status.unwrap_or("undefined"))),
            None => status.map(str::to_string),
        };
        let (stop_reason, error_message) = map_stop_reason(status, incomplete_reason)?;
        self.output.stop_reason = stop_reason;
        self.output.error_message = error_message;
        if self.has_tool_call() && self.output.stop_reason == StopReason::Stop {
            self.output.stop_reason = StopReason::ToolUse;
        }
        Ok(())
    }

    fn has_tool_call(&self) -> bool {
        self.output
            .content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_)))
    }

    /// 终态编码(对齐 TS stream() 尾部 + catch 分支):
    /// 流错误/中止 → error(pending / error 停止原因 → error),否则 done。
    fn finish(
        self,
        aborted: bool,
        stream_error: Option<String>,
    ) -> (AssistantMessageEvent, AssistantMessage) {
        let ResponsesAggregator {
            output: mut message,
            ..
        } = self;
        if let Some(error) = stream_error {
            let reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            message.stop_reason = reason;
            message.error_message = Some(error);
            return (error_event(reason, &message), message);
        }
        if aborted {
            message.stop_reason = StopReason::Aborted;
            message.error_message = Some("Request was aborted".to_string());
            return (error_event(StopReason::Aborted, &message), message);
        }
        if message.stop_reason == StopReason::Pending {
            message.stop_reason = StopReason::Error;
            message.error_message =
                Some("OpenAI Responses stream ended without a stop reason".to_string());
            return (error_event(StopReason::Error, &message), message);
        }
        if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
            if message.error_message.is_none() {
                message.error_message = Some("An unknown error occurred".to_string());
            }
            return (error_event(message.stop_reason, &message), message);
        }
        (
            AssistantMessageEvent::Done {
                reason: message.stop_reason,
                message: message.clone(),
            },
            message,
        )
    }
}

fn error_event(reason: StopReason, message: &AssistantMessage) -> AssistantMessageEvent {
    AssistantMessageEvent::Error {
        reason,
        error: message.clone(),
    }
}

/// TS mapStopReason:response.status → 统一 StopReason;未知 status 视为流错误。
fn map_stop_reason(
    status: Option<&str>,
    incomplete_reason: Option<&str>,
) -> Result<(StopReason, Option<String>), String> {
    match status {
        None => Ok((StopReason::Stop, None)),
        Some("completed") | Some("in_progress") | Some("queued") => Ok((StopReason::Stop, None)),
        Some("incomplete") => match incomplete_reason {
            Some("max_output_tokens") => Ok((StopReason::Length, None)),
            other => Ok((
                StopReason::Error,
                Some(match other {
                    Some(reason) => format!("Response incomplete: {reason}"),
                    None => "Response incomplete without a provider reason".to_string(),
                }),
            )),
        },
        // 蓝本对 failed/cancelled 不带 errorMessage,由外层兜底文案
        Some("failed") | Some("cancelled") => Ok((StopReason::Error, None)),
        Some(other) => Err(format!("Unhandled stop reason: {other}")),
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

// ── 流式入口 ──────────────────────────────────────────────────────────

/// OpenAI Responses 流式生成:返回事件流(先 `start`,终止于 `done`/`error`)。
/// 失败/中止编码为 stopReason error/aborted 的最终消息,不 panic;
/// `signal` 取消即时生效(连接期与读取期)。
pub fn stream_openai_responses(
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
    let mut aggregator = ResponsesAggregator::new(&model);
    writer.push(AssistantMessageEvent::Start {
        partial: aggregator.output().clone(),
    });

    if signal.as_ref().is_some_and(|token| token.is_cancelled()) {
        let (event, message) = aggregator.finish(true, None);
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

    // Provider 内层重试:每次重试重新发请求;取消可打断退避睡眠。
    let connected = if let Some(token) = &signal {
        tokio::select! {
            result = send_with_retry(&model, options.as_ref(), &body, Some(token)) => result,
            _ = token.cancelled() => Err(("Request was aborted".to_string(), true)),
        }
    } else {
        send_with_retry(&model, options.as_ref(), &body, None).await
    };
    let mut response = match connected {
        Ok(response) => response,
        Err((message, aborted)) => {
            let (event, message) = aggregator.finish(aborted, Some(message));
            writer.push(event);
            writer.end(message);
            return;
        }
    };

    // TS onResponse:仅在重试收敛后的最终成功响应上回调一次(失败的中间尝试不回调)
    if let Some(on_response) = options
        .as_ref()
        .and_then(|options| options.on_response.as_ref())
    {
        let mut headers = HashMap::new();
        for (name, value) in response.headers() {
            if let Ok(value) = value.to_str() {
                headers.insert(name.as_str().to_string(), value.to_string());
            }
        }
        on_response(&super::types::ProviderResponse {
            status: response.status().as_u16(),
            headers,
        });
    }

    let mut decoder = SseDecoder::new();
    let mut aborted = false;
    let mut stream_error: Option<String> = None;
    'read: loop {
        let item = if let Some(token) = &signal {
            tokio::select! {
                item = response.chunk() => item,
                _ = token.cancelled() => {
                    aborted = true;
                    break 'read;
                }
            }
        } else {
            response.chunk().await
        };
        match item {
            Ok(Some(chunk)) => {
                for payload in decoder.push(&chunk) {
                    // 忽略非 JSON data 行(如部分网关的 [DONE] 哨兵)
                    let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                        continue;
                    };
                    match aggregator.apply_event(&event) {
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
            Ok(None) => break 'read,
            Err(error) => {
                stream_error = Some(error.to_string());
                break 'read;
            }
        }
    }
    // 流正常结束但 signal 已中止(对齐 TS 循环后的 signal.aborted 检查)
    if stream_error.is_none()
        && !aborted
        && signal.as_ref().is_some_and(|token| token.is_cancelled())
    {
        aborted = true;
    }
    // TS:未见到终态 response 事件视为流错误
    if stream_error.is_none() && !aggregator.saw_terminal {
        stream_error =
            Some("OpenAI Responses stream ended before a terminal response event".to_string());
    }

    let (event, message) = aggregator.finish(aborted, stream_error);
    writer.push(event);
    writer.end(message);
}

// ── Provider 内层重试(TS utils/provider-retry.ts) ────────────────────

/// 单次请求失败:错误文案 + 重试判定所需的服务端提示。
struct RequestFailure {
    message: String,
    retryable: bool,
    /// `retry-after-ms` 原始值(毫秒浮点)
    retry_after_ms: Option<String>,
    /// `retry-after` 原始值(秒数或 HTTP 日期)
    retry_after: Option<String>,
}

/// TS isRetryableProviderError:`x-should-retry` 优先(true 重试/false 不重试),
/// 否则无 status(传输层错误)或 408/409/429/5xx 可重试。
fn is_retryable_provider_error(status: Option<u16>, x_should_retry: Option<&str>) -> bool {
    match x_should_retry {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    match status {
        None => true,
        Some(status) => status == 408 || status == 409 || status == 429 || status >= 500,
    }
}

/// `retry-after-ms` 浮点毫秒;非法值忽略(TS parseFloat NaN 检查)。
fn parse_retry_after_ms(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// `retry-after`:秒数,或 HTTP 日期(IMF-fixdate,经 RFC 2822 解析)减当前时间。
fn parse_retry_after(value: &str, now_ms: i64) -> Option<f64> {
    let trimmed = value.trim();
    if let Ok(seconds) = trimmed.parse::<f64>() {
        return seconds.is_finite().then_some(seconds * 1000.0);
    }
    let target = chrono::DateTime::parse_from_rfc2822(trimmed).ok()?;
    Some((target.timestamp_millis() - now_ms) as f64)
}

/// 指数退避(秒 → 毫秒,8s 封顶)× 随机抖动(jitter ∈ [0,1],衰减至 75% 起)。
fn exponential_retry_delay_ms(retry_index: u32, jitter: f64) -> u64 {
    let base = (0.5 * 2f64.powi(retry_index.min(16) as i32)).min(8.0) * 1000.0;
    let jitter = jitter.clamp(0.0, 1.0);
    (base * (1.0 - jitter * 0.25)) as u64
}

/// TS getRetryDelayMs + validateServerRetryDelayMs:服务端提示延迟优先
/// (retry-after-ms → retry-after),不做抖动;超过 `max_retry_delay_ms`
/// (缺省 60s,0 = 不限制)立即失败,文案对齐 TS;否则指数退避 + 抖动。
/// Err 文案即为流内错误消息。
fn next_retry_delay_ms(
    retry_after_ms: Option<&str>,
    retry_after: Option<&str>,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
    now_ms: i64,
    jitter: f64,
    provider_error_message: &str,
) -> Result<u64, String> {
    let validate = |delay_ms: f64| -> Result<u64, String> {
        let max_delay = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS) as f64;
        if max_delay > 0.0 && delay_ms > max_delay {
            return Err(format!(
                "Server requested {}s retry delay (max: {}s). {}",
                (delay_ms / 1000.0).ceil(),
                (max_delay / 1000.0).ceil(),
                provider_error_message
            ));
        }
        Ok(delay_ms.max(0.0) as u64)
    };
    if let Some(delay_ms) = retry_after_ms.and_then(parse_retry_after_ms) {
        return validate(delay_ms);
    }
    if let Some(delay_ms) = retry_after.and_then(|value| parse_retry_after(value, now_ms)) {
        return validate(delay_ms);
    }
    Ok(exponential_retry_delay_ms(retry_index, jitter))
}

enum SleepWait {
    Elapsed,
    Aborted,
}

/// 退避睡眠;取消立即返回 [`SleepWait::Aborted`](对齐 TS abortableSleep)。
async fn sleep_or_cancel(delay_ms: u64, signal: Option<&CancellationToken>) -> SleepWait {
    let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
    match signal {
        None => {
            sleep.await;
            SleepWait::Elapsed
        }
        Some(token) => tokio::select! {
            _ = sleep => SleepWait::Elapsed,
            _ = token.cancelled() => SleepWait::Aborted,
        },
    }
}

/// 时间派生的伪随机抖动量(指数退避防惊群用,质量不作要求)。
fn pseudo_random_jitter() -> f64 {
    (now_ts_nanos() % 1_000_000) as f64 / 1_000_000.0
}

/// TS retryProviderRequest:循环重试连接请求。每次重试都重新构造并发送
/// 全新请求;`options.max_retries` 缺省 2(OpenAI SDK 一致),可重试错误见
/// [`is_retryable_provider_error`];服务端延迟超限立即失败;退避睡眠可被
/// `signal` 中断(中断/发送途中取消 → ("Request was aborted", true))。
/// 返回 Err 的 bool = 是否因取消而失败。
async fn send_with_retry(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
    body: &Value,
    signal: Option<&CancellationToken>,
) -> Result<reqwest::Response, (String, bool)> {
    let api_key = resolve_api_key(model, options).map_err(|message| (message, false))?;
    let max_retries = options
        .and_then(|options| options.max_retries)
        .unwrap_or(DEFAULT_PROVIDER_MAX_RETRIES);
    let max_retry_delay_ms = options.and_then(|options| options.max_retry_delay_ms);
    let mut retries_remaining = max_retries;
    let mut retry_index: u32 = 0;

    loop {
        match send_responses_request(model, options, body, &api_key).await {
            Ok(response) => return Ok(response),
            Err(failure) => {
                if signal.is_some_and(|token| token.is_cancelled()) {
                    return Err(("Request was aborted".to_string(), true));
                }
                if retries_remaining == 0 || !failure.retryable {
                    return Err((failure.message, false));
                }
                retries_remaining -= 1;
                let now_ms = now_ts_nanos() / 1_000_000;
                let delay_ms = next_retry_delay_ms(
                    failure.retry_after_ms.as_deref(),
                    failure.retry_after.as_deref(),
                    retry_index,
                    max_retry_delay_ms,
                    now_ms,
                    pseudo_random_jitter(),
                    &failure.message,
                )
                .map_err(|message| (message, false))?;
                retry_index += 1;
                if let SleepWait::Aborted = sleep_or_cancel(delay_ms, signal).await {
                    return Err(("Request was aborted".to_string(), true));
                }
            }
        }
    }
}

/// 构造并发送 `POST {base_url}/responses` 请求,返回流式响应。
/// 非 2xx 时读取响应体格式化为 [`RequestFailure`](重试判定 + 服务端延迟提示);
/// 传输层错误按无 status 处理 = 可重试、无延迟提示。
async fn send_responses_request(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
    body: &Value,
    api_key: &str,
) -> Result<reqwest::Response, RequestFailure> {
    let failure = |message: String, retryable: bool| RequestFailure {
        message,
        retryable,
        retry_after_ms: None,
        retry_after: None,
    };
    let compat = get_compat(model);
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}")) {
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_str(&user_agent())
            .unwrap_or(reqwest::header::HeaderValue::from_static("pi-repomeow")),
    );
    // 会话亲和头:缓存关闭时不携带(与 prompt_cache_key 同一开关)
    let cache_retention = options
        .and_then(|options| options.cache_retention)
        .unwrap_or(CacheRetention::Short);
    let cache_session_id = if cache_retention == CacheRetention::None {
        None
    } else {
        options.and_then(|options| options.session_id.clone())
    };
    if let Some(session_id) = &cache_session_id {
        apply_session_affinity_headers(&mut headers, &compat, session_id);
    }
    // model.headers / options.headers 最后合并,可覆盖默认
    push_custom_headers(model.headers.as_ref(), &mut headers);
    push_custom_headers(
        options.and_then(|options| options.headers.as_ref()),
        &mut headers,
    );

    let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(15));
    if let Some(timeout_ms) = options.and_then(|options| options.timeout_ms) {
        builder = builder.timeout(Duration::from_millis(timeout_ms));
    }
    let http = builder.build().map_err(|error| {
        // 客户端构造失败与环境相关,重试无意义
        failure(format!("failed to build HTTP client: {error}"), false)
    })?;
    let url = format!("{}/responses", model.base_url.trim_end_matches('/'));
    let response = http
        .post(&url)
        .headers(headers)
        .json(body)
        .send()
        .await
        // 传输层错误无 status:对齐 TS 视为可重试
        .map_err(|error| failure(error.to_string(), true))?;

    let status = response.status();
    if !status.is_success() {
        let header_value = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let retry_after_ms = header_value("retry-after-ms");
        let retry_after = header_value("retry-after");
        let should_retry = header_value("x-should-retry");
        let retryable = is_retryable_provider_error(Some(status.as_u16()), should_retry.as_deref());
        let body_text = response.text().await.unwrap_or_default();
        return Err(RequestFailure {
            message: format_http_error(status.as_u16(), &body_text),
            retryable,
            retry_after_ms,
            retry_after,
        });
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::types::{
        ModelCost, ModelCostRates, OpenAICompletionsCompat, ThinkingLevel, UserMessage,
    };
    use serde_json::json;

    fn test_model(base_url: &str) -> Model {
        Model::from_settings("gpt-test", base_url)
    }

    fn reasoning_model(base_url: &str) -> Model {
        let mut model = test_model(base_url);
        model.reasoning = true;
        model
    }

    fn user_message(text: &str) -> Message {
        Message::User(UserMessage {
            role: "user".to_string(),
            content: UserContent::text(text),
            timestamp: 0,
        })
    }

    fn assistant_message(content: Vec<AssistantContent>, stop_reason: StopReason) -> Message {
        // provider/api/model 与 Model::from_settings 对齐 = 同模型回放语义
        Message::Assistant(AssistantMessage {
            role: "assistant".to_string(),
            content,
            api: "openai-completions".to_string(),
            provider: "custom".to_string(),
            model: "gpt-test".to_string(),
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

    fn tool_call(id: &str, name: &str, arguments: Value) -> AssistantContent {
        AssistantContent::ToolCall(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: serde_json::from_value(arguments).unwrap(),
            thought_signature: None,
            namespace: None,
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

    fn aggregator_for(base_url: &str) -> ResponsesAggregator {
        ResponsesAggregator::new(&test_model(base_url))
    }

    /// 合并 `{"type": ...}` 与附加字段的事件 JSON。
    fn ev(event_type: &str, extra: Value) -> Value {
        let mut object = json!({ "type": event_type });
        if let (Some(map), Some(extra)) = (object.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                map.insert(key.clone(), value.clone());
            }
        }
        object
    }

    // ── build_request_body ───────────────────────────────────────────

    #[test]
    fn basic_body_shape_and_developer_role() {
        let model = reasoning_model("https://api.openai.com/v1");
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());
        let body = build_request_body(&model, &context, None);

        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        // reasoning 模型 + 默认 supportsDeveloperRole → developer
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["input"][0]["content"], "be brief");
        assert_eq!(body["input"][1]["role"], "user");
        assert_eq!(body["input"][1]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][1]["content"][0]["text"], "hi");
        // 未给 reasoning 级别:off 映射缺省 → effort "none"
        assert_eq!(body["reasoning"]["effort"], "none");
        assert!(body.get("include").is_none());
        // 无 max_tokens 且 model.max_tokens = 0 → 不下发
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn system_role_falls_back_for_non_reasoning_or_compat_off() {
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());

        let plain = test_model("https://api.openai.com/v1");
        let body = build_request_body(&plain, &context, None);
        assert_eq!(body["input"][0]["role"], "system");

        let mut model = reasoning_model("https://api.openai.com/v1");
        model.compat = Some(OpenAICompletionsCompat {
            supports_developer_role: Some(false),
            ..Default::default()
        });
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["input"][0]["role"], "system");
    }

    #[test]
    fn prompt_cache_key_and_retention() {
        let model = test_model("https://api.openai.com/v1");
        let context = context_of(vec![user_message("hi")]);
        let options = SimpleStreamOptions {
            session_id: Some("sess-123".to_string()),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["prompt_cache_key"], "sess-123");
        assert!(body.get("prompt_cache_retention").is_none());

        // long retention → "24h"
        let options = SimpleStreamOptions {
            session_id: Some("sess".to_string()),
            cache_retention: Some(CacheRetention::Long),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["prompt_cache_retention"], "24h");

        // none:既无 cache key 也无 retention
        let options = SimpleStreamOptions {
            session_id: Some("sess".to_string()),
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("prompt_cache_retention").is_none());

        // 超 64 字符截断
        let options = SimpleStreamOptions {
            session_id: Some("s".repeat(80)),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["prompt_cache_key"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn max_output_tokens_clamped_and_floored() {
        let mut model = test_model("https://api.openai.com/v1");
        model.max_tokens = 100_000;
        model.context_window = 128_000;
        let context = context_of(vec![user_message("hi")]);
        let options = SimpleStreamOptions {
            max_tokens: Some(100),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["max_output_tokens"], 100);

        // 低于 16 抬到最小值
        let options = SimpleStreamOptions {
            max_tokens: Some(4),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["max_output_tokens"], 16);

        // model.max_tokens = 0 且未指定 → 不下发
        let bare = test_model("https://api.openai.com/v1");
        let body = build_request_body(&bare, &context, None);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn tools_flat_shape_and_strict_flag() {
        let model = test_model("https://api.openai.com/v1");
        let mut context = context_of(vec![user_message("hi")]);
        context.tools = vec![Tool {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
        }];
        let body = build_request_body(&model, &context, None);
        let tool = &body["tools"][0];
        // Responses 工具字段平铺(非 chat completions 的 function 嵌套)
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "get_weather");
        assert_eq!(tool["description"], "Get weather");
        assert!(tool.get("strict").is_none());

        let mut model = model;
        model.compat = Some(OpenAICompletionsCompat {
            supports_strict_mode: Some(true),
            ..Default::default()
        });
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["tools"][0]["strict"], false);
    }

    #[test]
    fn tool_choice_and_sampling_params() {
        let model = test_model("https://api.openai.com/v1");
        let context = context_of(vec![user_message("hi")]);
        let mut sampling = HashMap::new();
        sampling.insert("top_p".to_string(), json!(0.5));
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::None),
            temperature: Some(0.2),
            sampling_params: Some(sampling),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["tool_choice"], "none");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 0.5);
    }

    #[test]
    fn reasoning_effort_summary_and_include() {
        let model = reasoning_model("https://api.openai.com/v1");
        let context = context_of(vec![user_message("hi")]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));

        // thinkingLevelMap 覆盖 effort
        let mut model = model;
        let mut map: HashMap<String, Option<String>> = HashMap::new();
        map.insert("high".to_string(), Some("medium".to_string()));
        model.thinking_level_map = Some(map);
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["reasoning"]["effort"], "medium");

        // 就近钳制:Max → high(xhigh/max 未映射不可用)
        let plain = reasoning_model("https://api.openai.com/v1");
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Max),
            ..Default::default()
        };
        let body = build_request_body(&plain, &context, Some(&options));
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));

        // 无偏好:off 映射生效且不带 summary/include
        let mut map: HashMap<String, Option<String>> = HashMap::new();
        map.insert("off".to_string(), Some("low".to_string()));
        model.thinking_level_map = Some(map);
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert!(body.get("include").is_none());
        assert!(body["reasoning"].get("summary").is_none());

        // off 显式 null:不下发 reasoning
        let mut map: HashMap<String, Option<String>> = HashMap::new();
        map.insert("off".to_string(), None);
        model.thinking_level_map = Some(map);
        let body = build_request_body(&model, &context, None);
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn xai_always_includes_encrypted_content() {
        let mut model = reasoning_model("https://api.x.ai/v1");
        model.provider = "xai".to_string();
        let context = context_of(vec![user_message("hi")]);
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["reasoning"]["effort"], "none");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn non_reasoning_model_has_no_reasoning_params() {
        let model = test_model("https://api.openai.com/v1");
        let context = context_of(vec![user_message("hi")]);
        let body = build_request_body(&model, &context, None);
        assert!(body.get("reasoning").is_none());
        assert!(body.get("include").is_none());
    }

    // ── 消息转换 ─────────────────────────────────────────────────────

    #[test]
    fn user_blocks_become_input_items() {
        let mut model = test_model("https://api.openai.com/v1");
        model.input = vec![InputKind::Text, InputKind::Image];
        let messages = vec![Message::User(UserMessage {
            role: "user".to_string(),
            content: UserContent::Blocks(vec![
                TextOrImageContent::text("look"),
                TextOrImageContent::Image {
                    data: "QUJD".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ]),
            timestamp: 0,
        })];
        let body = build_request_body(&model, &context_of(messages), None);
        let parts = body["input"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "look");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["detail"], "auto");
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,QUJD");

        // 非图片模型:图片降级为占位文本
        let plain = test_model("https://api.openai.com/v1");
        let messages = vec![Message::User(UserMessage {
            role: "user".to_string(),
            content: UserContent::Blocks(vec![
                TextOrImageContent::text("look"),
                TextOrImageContent::Image {
                    data: "QUJD".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ]),
            timestamp: 0,
        })];
        let body = build_request_body(&plain, &context_of(messages), None);
        let parts = body["input"][0]["content"].as_array().unwrap();
        assert_eq!(parts[1]["type"], "input_text");
        assert_eq!(parts[1]["text"], USER_IMAGE_PLACEHOLDER);
    }

    #[test]
    fn assistant_text_replay_builds_message_items_with_signature() {
        let model = test_model("https://api.openai.com/v1");
        let text = AssistantContent::Text {
            text: "answer".to_string(),
            text_signature: Some(r#"{"v":1,"id":"msg_abc","phase":"final_answer"}"#.to_string()),
        };
        let messages = vec![
            user_message("q"),
            assistant_message(vec![text], StopReason::Stop),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        let item = &body["input"][1];
        assert_eq!(item["type"], "message");
        assert_eq!(item["role"], "assistant");
        assert_eq!(item["status"], "completed");
        assert_eq!(item["id"], "msg_abc");
        assert_eq!(item["phase"], "final_answer");
        assert_eq!(item["content"][0]["type"], "output_text");
        assert_eq!(item["content"][0]["text"], "answer");
        assert_eq!(item["content"][0]["annotations"], json!([]));

        // 无签名 → msg_pi_<index> 回退 id;第二段带 _<textBlockIndex> 后缀
        let text = AssistantContent::text("a");
        let second = AssistantContent::text("b");
        let messages = vec![
            user_message("q"),
            assistant_message(vec![text, second], StopReason::Stop),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        assert_eq!(body["input"][1]["id"], "msg_pi_1");
        assert_eq!(body["input"][2]["id"], "msg_pi_1_1");

        // 超长签名 id → msg_<shortHash>
        let long_id = "m".repeat(80);
        let text = AssistantContent::Text {
            text: "x".to_string(),
            text_signature: Some(json!({"v": 1, "id": long_id}).to_string()),
        };
        let messages = vec![assistant_message(vec![text], StopReason::Stop)];
        let body = build_request_body(&model, &context_of(messages), None);
        let id = body["input"][0]["id"].as_str().unwrap();
        assert_eq!(id, format!("msg_{}", short_hash(&long_id)));

        // legacy 纯字符串签名:整串即 id
        let text = AssistantContent::Text {
            text: "x".to_string(),
            text_signature: Some("legacy_id".to_string()),
        };
        let messages = vec![assistant_message(vec![text], StopReason::Stop)];
        let body = build_request_body(&model, &context_of(messages), None);
        assert_eq!(body["input"][0]["id"], "legacy_id");
        assert!(body["input"][0].get("phase").is_none());
    }

    #[test]
    fn assistant_thinking_replay_passes_signature_item_through() {
        let model = test_model("https://api.openai.com/v1");
        let reasoning_item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "hmm"}],
            "encrypted_content": "gAAAAA",
        });
        let thinking = AssistantContent::Thinking {
            thinking: "hmm".to_string(),
            thinking_signature: Some(reasoning_item.to_string()),
            redacted: false,
        };
        let messages = vec![
            user_message("q"),
            assistant_message(
                vec![thinking, AssistantContent::text("answer")],
                StopReason::Stop,
            ),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        assert_eq!(body["input"][1], reasoning_item);
        assert_eq!(body["input"][2]["type"], "message");
    }

    #[test]
    fn tool_call_replay_ids() {
        let model = test_model("https://api.openai.com/v1");
        // 同模型 + fc_ item id:保留 id
        let messages = vec![
            user_message("q"),
            assistant_message(
                vec![tool_call(
                    "call_1|fc_item1",
                    "get_weather",
                    json!({"city": "Oslo"}),
                )],
                StopReason::ToolUse,
            ),
            tool_result_message("call_1|fc_item1", "get_weather", "18C"),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        assert_eq!(body["input"][1]["type"], "function_call");
        assert_eq!(body["input"][1]["id"], "fc_item1");
        assert_eq!(body["input"][1]["call_id"], "call_1");
        assert_eq!(body["input"][1]["name"], "get_weather");
        assert_eq!(body["input"][1]["arguments"], r#"{"city":"Oslo"}"#);
        assert_eq!(body["input"][2]["type"], "function_call_output");
        assert_eq!(body["input"][2]["call_id"], "call_1");
        assert_eq!(body["input"][2]["output"], "18C");

        // 同模型但非 fc_ item id:id 丢弃,call_id 保留
        let messages = vec![
            assistant_message(
                vec![tool_call("call_2|ctc_item", "f", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result_message("call_2|ctc_item", "f", "ok"),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        assert!(body["input"][0].get("id").is_none());
        assert_eq!(body["input"][0]["call_id"], "call_2");
        assert_eq!(body["input"][1]["call_id"], "call_2");

        // 不同模型 + fc_ id:id 丢弃(避免 fc↔rs 配对校验)
        let mut cross_model = assistant_message(
            vec![tool_call("call_3|fc_item3", "f", json!({}))],
            StopReason::ToolUse,
        );
        if let Message::Assistant(assistant) = &mut cross_model {
            assistant.model = "other".to_string();
        }
        let messages = vec![
            cross_model,
            tool_result_message("call_3|fc_item3", "f", "ok"),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        assert!(body["input"][0].get("id").is_none());

        // 无管道分隔的 id:call_id = 整串,无 item id
        let messages = vec![assistant_message(
            vec![tool_call("call_plain", "f", json!({}))],
            StopReason::ToolUse,
        )];
        let body = build_request_body(&model, &context_of(messages), None);
        assert_eq!(body["input"][0]["call_id"], "call_plain");
        assert!(body["input"][0].get("id").is_none());
    }

    #[test]
    fn tool_result_with_images_and_placeholders() {
        let mut model = test_model("https://api.openai.com/v1");
        model.input = vec![InputKind::Text, InputKind::Image];
        let messages = vec![Message::ToolResult(ToolResultMessage {
            role: "toolResult".to_string(),
            tool_call_id: "call_1".to_string(),
            tool_name: "shot".to_string(),
            content: vec![
                TextOrImageContent::text("screenshot"),
                TextOrImageContent::Image {
                    data: "QUJD".to_string(),
                    mime_type: "image/jpeg".to_string(),
                },
            ],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        })];
        let body = build_request_body(&model, &context_of(messages.clone()), None);
        let output = &body["input"][0]["output"];
        assert!(output.is_array());
        assert_eq!(output[0]["type"], "input_text");
        assert_eq!(output[0]["text"], "screenshot");
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(output[1]["image_url"], "data:image/jpeg;base64,QUJD");

        // 非图片模型:transform 先把图片降级为占位文本,合并为纯文本
        let plain = test_model("https://api.openai.com/v1");
        let body = build_request_body(&plain, &context_of(messages), None);
        assert_eq!(
            body["input"][0]["output"],
            format!("screenshot\n{TOOL_IMAGE_PLACEHOLDER}")
        );

        // 图片能力模型 + 仅图片无文本:无文本段,只输出图片项
        let messages = vec![Message::ToolResult(ToolResultMessage {
            role: "toolResult".to_string(),
            tool_call_id: "call_1".to_string(),
            tool_name: "shot".to_string(),
            content: vec![TextOrImageContent::Image {
                data: "QUJD".to_string(),
                mime_type: "image/jpeg".to_string(),
            }],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        })];
        let body = build_request_body(&model, &context_of(messages), None);
        let output = &body["input"][0]["output"];
        assert!(output.is_array());
        assert_eq!(output[0]["type"], "input_image");

        // 纯函数分支:图片存在但模型不支持 → 占位字符串(整链路上 transform 先降级,
        // 该分支仅在直接调用时触达)
        let fallback = convert_tool_result_output(
            &plain,
            &[TextOrImageContent::Image {
                data: "QUJD".to_string(),
                mime_type: "image/jpeg".to_string(),
            }],
        );
        assert_eq!(fallback, json!(TOOL_RESULT_IMAGE_TEXT));

        // 空输出:占位符
        let messages = vec![Message::ToolResult(ToolResultMessage {
            role: "toolResult".to_string(),
            tool_call_id: "call_1".to_string(),
            tool_name: "f".to_string(),
            content: vec![TextOrImageContent::text("")],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        })];
        let body = build_request_body(&plain, &context_of(messages), None);
        assert_eq!(body["input"][0]["output"], TOOL_RESULT_EMPTY_TEXT);
    }

    #[test]
    fn errored_assistant_skipped_and_orphan_tool_call_gets_synthetic_result() {
        let model = test_model("https://api.openai.com/v1");
        let messages = vec![
            user_message("hi"),
            assistant_message(vec![], StopReason::Error),
            assistant_message(
                vec![tool_call("call_9", "get_weather", json!({}))],
                StopReason::ToolUse,
            ),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        let input = body["input"].as_array().unwrap();
        // user + function_call + 合成的 function_call_output
        assert_eq!(input.len(), 3);
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["output"], SYNTHETIC_TOOL_RESULT_TEXT);
    }

    #[test]
    fn cross_provider_tool_call_ids_are_rebuilt() {
        // 本方为 openai 系 provider:跨提供方(api 不同)→ item id 重铸为 fc_<hash>
        let mut model = test_model("https://api.openai.com/v1");
        model.provider = "openai".to_string();
        let piped = "call|item_with_specials!@#";
        let mut foreign =
            assistant_message(vec![tool_call(piped, "f", json!({}))], StopReason::ToolUse);
        if let Message::Assistant(assistant) = &mut foreign {
            assistant.api = "anthropic-messages".to_string();
        }
        let messages = vec![foreign, tool_result_message(piped, "f", "ok")];
        let body = build_request_body(&model, &context_of(messages), None);
        let item_id = body["input"][0]["id"].as_str().unwrap();
        assert!(item_id.starts_with("fc_"));
        assert_eq!(body["input"][1]["call_id"], "call");

        // 本方非 openai 系 provider:id 整体消毒(管道变 _、尾部下划线去除)
        let model = test_model("https://example.invalid/v1");
        let mut foreign = assistant_message(
            vec![tool_call("call|item!@", "f", json!({}))],
            StopReason::ToolUse,
        );
        if let Message::Assistant(assistant) = &mut foreign {
            assistant.model = "other".to_string();
        }
        let messages = vec![foreign, tool_result_message("call|item!@", "f", "ok")];
        let body = build_request_body(&model, &context_of(messages), None);
        assert_eq!(body["input"][0]["type"], "function_call");
        assert!(body["input"][0].get("id").is_none());
        assert_eq!(body["input"][0]["call_id"], "call_item");
        // toolResult 的 id 经映射表同步
        assert_eq!(body["input"][1]["call_id"], "call_item");
    }

    // ── 工具函数 ─────────────────────────────────────────────────────

    #[test]
    fn normalize_id_part_sanifies_truncates_and_trims() {
        assert_eq!(normalize_id_part("abc_DEF-123"), "abc_DEF-123");
        assert_eq!(normalize_id_part("a b!c"), "a_b_c");
        assert_eq!(normalize_id_part(&"x".repeat(70)), "x".repeat(64));
        assert_eq!(normalize_id_part("abc___"), "abc");
        assert_eq!(normalize_id_part(""), "");
    }

    #[test]
    fn text_signature_roundtrip() {
        let encoded = encode_text_signature_v1("msg_1", Some("commentary"));
        let (id, phase) = parse_text_signature(&encoded);
        assert_eq!(id, "msg_1");
        assert_eq!(phase.as_deref(), Some("commentary"));

        // 非法 phase 丢弃
        let (id, phase) = parse_text_signature(r#"{"v":1,"id":"msg_1","phase":"weird"}"#);
        assert_eq!(id, "msg_1");
        assert!(phase.is_none());

        // v 缺失 → 整串作为 legacy id
        let (id, _) = parse_text_signature(r#"{"id":"x"}"#);
        assert_eq!(id, r#"{"id":"x"}"#);

        // 非 JSON → 整串
        let (id, _) = parse_text_signature("plain");
        assert_eq!(id, "plain");
    }

    #[test]
    fn short_hash_stable_and_matches_ts_shape() {
        let first = short_hash("call|item");
        assert_eq!(first, short_hash("call|item"));
        assert!(!first.is_empty());
        assert!(first
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    // ── SSE 解码 ─────────────────────────────────────────────────────

    #[test]
    fn sse_decoder_handles_split_chunks_and_crlf() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: {\"type\":\"resp").is_empty());
        let events = decoder.push(b"onse.created\"}\r\n\r\ndata: {\"type\":\"response.done\"}\n\n");
        assert_eq!(
            events,
            vec![
                "{\"type\":\"response.created\"}",
                "{\"type\":\"response.done\"}"
            ]
        );

        // 多行 data 以 \n 合并;event:/注释行忽略
        let mut decoder = SseDecoder::new();
        let events =
            decoder.push(b": keep-alive\nevent: response.created\ndata: first\ndata: second\n\n");
        assert_eq!(events, vec!["first\nsecond"]);
    }

    // ── 流聚合器 ─────────────────────────────────────────────────────

    #[test]
    fn text_stream_produces_ordered_events_and_finalizes() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let mut events = aggregator
            .apply_event(&ev(
                "response.created",
                json!({"response": {"id": "resp_1"}}),
            ))
            .unwrap();
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.output_item.added",
                    json!({"output_index": 0, "item": {"type": "message", "role": "assistant"}}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.output_text.delta",
                    json!({"output_index": 0, "delta": "Hel"}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.output_text.delta",
                    json!({"output_index": 0, "delta": "lo"}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.output_item.done",
                    json!({"output_index": 0, "item": {"type": "message", "role": "assistant", "id": "msg_1", "status": "completed", "content": [{"type": "output_text", "text": "Hello", "annotations": []}]}}),
                ))
                .unwrap(),
        );
        aggregator
            .finalize_response(&json!({
                "id": "resp_1",
                "status": "completed",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 5,
                    "total_tokens": 105,
                    "input_tokens_details": {"cached_tokens": 60},
                    "output_tokens_details": {"reasoning_tokens": 2}
                }
            }))
            .unwrap();

        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AssistantMessageEvent::TextStart { .. } => "text_start",
                AssistantMessageEvent::TextDelta { delta, .. } => {
                    assert!(!delta.is_empty());
                    "text_delta"
                }
                AssistantMessageEvent::TextEnd { content, .. } => {
                    assert_eq!(content, "Hello");
                    "text_end"
                }
                other => unreachable!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["text_start", "text_delta", "text_delta", "text_end"]
        );

        let (terminal, message) = aggregator.finish(false, None);
        assert_eq!(message.response_id.as_deref(), Some("resp_1"));
        // item done 会写入 TextSignatureV1 签名
        let expected = AssistantContent::Text {
            text: "Hello".to_string(),
            text_signature: Some(encode_text_signature_v1("msg_1", None)),
        };
        assert_eq!(message.content[0], expected);
        match terminal {
            AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Stop),
            other => unreachable!("{other:?}"),
        }
        // usage:input 扣缓存命中,reasoning 子集保留
        assert_eq!(message.usage.input, 40);
        assert_eq!(message.usage.cache_read, 60);
        assert_eq!(message.usage.output, 5);
        assert_eq!(message.usage.total_tokens, 105);
        assert_eq!(message.usage.reasoning, Some(2));
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("completed"));
    }

    #[test]
    fn reasoning_summary_streams_then_item_done_replaces_text() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let mut events = aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "reasoning", "id": "rs_1"}}),
            ))
            .unwrap();
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.reasoning_summary_text.delta",
                    json!({"output_index": 0, "delta": "part one"}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.reasoning_summary_part.done",
                    json!({"output_index": 0}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.reasoning_summary_text.delta",
                    json!({"output_index": 0, "delta": "part two"}),
                ))
                .unwrap(),
        );
        let done_item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "summary": [{"type": "summary_text", "text": "part one"}, {"type": "summary_text", "text": "part two"}],
            "encrypted_content": "gAAAAA"
        });
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.output_item.done",
                    json!({"output_index": 0, "item": done_item}),
                ))
                .unwrap(),
        );

        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
                AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                    assert!(!delta.is_empty());
                    "thinking_delta"
                }
                AssistantMessageEvent::ThinkingEnd { content, .. } => {
                    // item done 用 summary 拼接替换累计文本
                    assert_eq!(content, "part one\n\npart two");
                    "thinking_end"
                }
                other => unreachable!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "thinking_start",
                "thinking_delta",
                "thinking_delta",
                "thinking_delta",
                "thinking_end"
            ]
        );

        let message = aggregator.finish(false, None).1;
        let AssistantContent::Thinking {
            thinking,
            thinking_signature,
            ..
        } = &message.content[0]
        else {
            panic!()
        };
        assert_eq!(thinking, "part one\n\npart two");
        // 签名 = 完整 reasoning item JSON
        let stored: Value = serde_json::from_str(thinking_signature.as_deref().unwrap()).unwrap();
        assert_eq!(stored["id"], "rs_1");
        assert_eq!(stored["encrypted_content"], "gAAAAA");
    }

    #[test]
    fn tool_call_aggregates_partial_json_and_emits_tail_delta() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let mut events = aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "get_weather", "arguments": ""}}),
            ))
            .unwrap();
        // 增量为最终参数的严格前缀(done 时补发剩余差量)
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 0, "delta": "{\"city\":"}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.function_call_arguments.delta",
                    json!({"output_index": 0, "delta": " \"Oslo\""}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.function_call_arguments.done",
                    json!({"output_index": 0, "arguments": "{\"city\": \"Oslo\"}"}),
                ))
                .unwrap(),
        );
        events.extend(
            aggregator
                .apply_event(&ev(
                    "response.output_item.done",
                    json!({"output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\": \"Oslo\"}"}}),
                ))
                .unwrap(),
        );
        aggregator
            .finalize_response(&json!({"status": "completed"}))
            .unwrap();

        // start + delta×2 + done 补发差量 + end
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
                AssistantMessageEvent::ToolcallDelta { delta, .. } => {
                    assert!(!delta.is_empty());
                    "toolcall_delta"
                }
                AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
                other => unreachable!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "toolcall_start",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_delta",
                "toolcall_end"
            ]
        );

        let (terminal, message) = aggregator.finish(false, None);
        let Some(AssistantMessageEvent::ToolcallEnd { tool_call, .. }) = events
            .iter()
            .find(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. }))
        else {
            panic!("expected toolcall_end");
        };
        assert_eq!(tool_call.id, "call_1|fc_1");
        assert_eq!(tool_call.name, "get_weather");
        assert_eq!(
            tool_call.arguments.get("city").and_then(Value::as_str),
            Some("Oslo")
        );
        // tool call 存在:stop → toolUse
        match terminal {
            AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::ToolUse),
            other => unreachable!("{other:?}"),
        }
        assert_eq!(message.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn terminal_event_backfills_missing_encrypted_content() {
        let model = test_model("https://api.openai.com/v1");
        let mut aggregator = ResponsesAggregator::new(&model);
        aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "reasoning", "id": "rs_1"}}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.output_item.done",
                json!({"output_index": 0, "item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
            ))
            .unwrap();
        // 初始签名无 encrypted_content
        let AssistantContent::Thinking {
            thinking_signature, ..
        } = &aggregator.output.content[0]
        else {
            panic!()
        };
        let stored: Value = serde_json::from_str(thinking_signature.as_deref().unwrap()).unwrap();
        assert!(stored.get("encrypted_content").is_none());

        // 终态 response.output 提供 encrypted_content → 回填
        aggregator
            .finalize_response(&json!({
                "id": "resp_1",
                "status": "completed",
                "output": [
                    {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "gAAAAA"}
                ]
            }))
            .unwrap();
        let AssistantContent::Thinking {
            thinking_signature, ..
        } = &aggregator.output.content[0]
        else {
            panic!()
        };
        let stored: Value = serde_json::from_str(thinking_signature.as_deref().unwrap()).unwrap();
        assert_eq!(stored["encrypted_content"], "gAAAAA");
    }

    #[test]
    fn incomplete_response_maps_length_for_max_output_tokens() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "message", "role": "assistant"}}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.output_item.done",
                json!({"output_index": 0, "item": {"type": "message", "id": "msg_1", "status": "incomplete", "content": [{"type": "output_text", "text": "trunca"}]}}),
            ))
            .unwrap();
        aggregator
            .finalize_response(&json!({
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"}
            }))
            .unwrap();
        let (terminal, message) = aggregator.finish(false, None);
        assert_eq!(message.stop_reason, StopReason::Length);
        assert_eq!(
            message.raw_stop_reason.as_deref(),
            Some("incomplete.max_output_tokens")
        );
        match terminal {
            AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::Length),
            other => unreachable!("{other:?}"),
        }

        // 其他 incomplete 原因 → error + 文案
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .finalize_response(&json!({
                "status": "incomplete",
                "incomplete_details": {"reason": "content_filter"}
            }))
            .unwrap();
        let (terminal, message) = aggregator.finish(false, None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Response incomplete: content_filter")
        );
        match terminal {
            AssistantMessageEvent::Error { reason, .. } => assert_eq!(reason, StopReason::Error),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn final_answer_phase_forces_stop() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "message", "role": "assistant", "phase": "final_answer"}}),
            ))
            .unwrap();
        assert_eq!(aggregator.output.stop_reason, StopReason::Stop);
    }

    #[test]
    fn error_event_and_failed_response_become_stream_errors() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let error = aggregator
            .apply_event(&ev(
                "error",
                json!({"code": "server_error", "message": "boom"}),
            ))
            .unwrap_err();
        assert_eq!(error, "Error Code server_error: boom");

        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let error = aggregator
            .apply_event(&ev(
                "response.failed",
                json!({"response": {"status": "failed", "error": {"code": "rate_limit", "message": "slow down"}}}),
            ))
            .unwrap_err();
        assert_eq!(error, "rate_limit: slow down");
        assert_eq!(aggregator.output.raw_stop_reason.as_deref(), Some("failed"));

        // 无错误详情的 failed → 兜底文案
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let error = aggregator
            .apply_event(&ev(
                "response.failed",
                json!({"response": {"status": "failed"}}),
            ))
            .unwrap_err();
        assert_eq!(error, "Unknown error (no error details in response)");
    }

    #[test]
    fn failed_status_finishes_as_error_with_fallback_message() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .finalize_response(&json!({"status": "failed"}))
            .unwrap();
        let (terminal, message) = aggregator.finish(false, None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("An unknown error occurred")
        );
        match terminal {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Error);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("An unknown error occurred")
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn missing_terminal_event_becomes_stream_error() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "message", "role": "assistant"}}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.output_text.delta",
                json!({"output_index": 0, "delta": "partial"}),
            ))
            .unwrap();
        let (terminal, message) = aggregator.finish(
            false,
            Some("OpenAI Responses stream ended before a terminal response event".to_string()),
        );
        assert!(message.error_message.is_some());
        match terminal {
            AssistantMessageEvent::Error { reason, .. } => assert_eq!(reason, StopReason::Error),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn aborted_finish_keeps_partial_content() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "message", "role": "assistant"}}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.output_text.delta",
                json!({"output_index": 0, "delta": "partial"}),
            ))
            .unwrap();
        let (terminal, message) = aggregator.finish(true, None);
        assert_eq!(message.content[0], AssistantContent::text("partial"));
        match terminal {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn unknown_status_becomes_stream_error() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let error = aggregator
            .finalize_response(&json!({"status": "mysterious"}))
            .unwrap_err();
        assert_eq!(error, "Unhandled stop reason: mysterious");
    }

    #[test]
    fn usage_absent_still_computes_zero_cost() {
        let mut model = test_model("https://api.openai.com/v1");
        model.cost = ModelCost {
            rates: ModelCostRates {
                input: 1.0,
                output: 2.0,
                cache_read: 0.5,
                cache_write: 0.0,
            },
            tiers: None,
        };
        let mut aggregator = ResponsesAggregator::new(&model);
        aggregator
            .finalize_response(&json!({"status": "completed"}))
            .unwrap();
        let (_, message) = aggregator.finish(false, None);
        assert_eq!(message.usage.input, 0);
        assert_eq!(message.usage.cost.total, 0.0);
    }

    #[test]
    fn custom_tool_call_input_streams_raw_text() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator
            .apply_event(&ev(
                "response.output_item.added",
                json!({"output_index": 0, "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "f", "input": "he"}}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.custom_tool_call_input.delta",
                json!({"output_index": 0, "delta": "llo"}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.custom_tool_call_input.done",
                json!({"output_index": 0, "input": "hello"}),
            ))
            .unwrap();
        aggregator
            .apply_event(&ev(
                "response.output_item.done",
                json!({"output_index": 0, "item": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1", "name": "f", "input": "hello"}}),
            ))
            .unwrap();
        let (_, message) = aggregator.finish(false, None);
        let AssistantContent::ToolCall(tool_call) = &message.content[0] else {
            panic!()
        };
        assert_eq!(tool_call.id, "call_1|ctc_1");
        assert_eq!(
            tool_call.arguments.get("input").and_then(Value::as_str),
            Some("hello")
        );
    }

    #[test]
    fn missing_start_event_creates_slot_on_item_done() {
        // output_item.done 先于 added(或缺失)到达:补建槽并发出 start 事件
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let events = aggregator
            .apply_event(&ev(
                "response.output_item.done",
                json!({"output_index": 0, "item": {"type": "message", "id": "msg_1", "status": "completed", "content": [{"type": "output_text", "text": "hi"}]}}),
            ))
            .unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AssistantMessageEvent::TextStart { .. } => "text_start",
                AssistantMessageEvent::TextEnd { .. } => "text_end",
                other => unreachable!("{other:?}"),
            })
            .collect();
        assert_eq!(kinds, vec!["text_start", "text_end"]);
    }

    // ── 入口冒烟(取消路径,不发网络请求) ─────────────────────────────

    #[tokio::test]
    async fn stream_entry_encodes_pre_cancelled_request_as_aborted() {
        use futures::StreamExt;

        let token = CancellationToken::new();
        token.cancel();
        let model = test_model("https://127.0.0.1:9/v1");
        let mut stream = stream_openai_responses(
            model,
            context_of(vec![user_message("hi")]),
            None,
            Some(token),
        );

        let mut saw_start = false;
        let mut terminal_seen: Option<AssistantMessageEvent> = None;
        while let Some(event) = stream.next().await {
            match event {
                AssistantMessageEvent::Start { .. } => saw_start = true,
                other => terminal_seen = Some(other),
            }
        }
        assert!(saw_start);
        match terminal_seen {
            Some(AssistantMessageEvent::Error { reason, error }) => {
                assert_eq!(reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => unreachable!("{other:?}"),
        }
        // 终值与终止事件一致
        let result = stream.result().await;
        assert_eq!(result.stop_reason, StopReason::Aborted);
    }

    #[test]
    fn format_http_error_truncates_body() {
        assert_eq!(
            format_http_error(429, "{\"error\":{\"message\":\"rate limited\"}}"),
            "OpenAI API error (429): {\"error\":{\"message\":\"rate limited\"}}"
        );
        let long = "x".repeat(5000);
        let message = format_http_error(500, &long);
        assert!(message.starts_with("OpenAI API error (500): "));
        assert!(message.contains("[truncated 1000 chars]"));
        assert_eq!(format_http_error(500, "   "), "OpenAI API error (500)");
    }

    // ── Provider 内层重试 ─────────────────────────────────────────────

    #[test]
    fn retryable_error_classification() {
        let retryable = |status: Option<u16>, should_retry: Option<&str>| {
            is_retryable_provider_error(status, should_retry)
        };
        // 无 status(传输层错误)与 408/409/429/5xx 可重试
        assert!(retryable(None, None));
        for status in [408u16, 409, 429, 500, 502, 503] {
            assert!(retryable(Some(status), None), "status {status}");
        }
        for status in [400u16, 401, 403, 404, 422] {
            assert!(!retryable(Some(status), None), "status {status}");
        }
        // x-should-retry 优先于 status 判定
        assert!(retryable(Some(400), Some("true")));
        assert!(!retryable(Some(500), Some("false")));
        assert!(retryable(None, Some("true")));
        assert!(!retryable(None, Some("false")));
    }

    #[test]
    fn retry_delay_parsing_and_validation() {
        let delay = |retry_after_ms: Option<&str>,
                     retry_after: Option<&str>,
                     index: u32,
                     max: Option<u64>,
                     now_ms: i64,
                     jitter: f64| {
            next_retry_delay_ms(
                retry_after_ms,
                retry_after,
                index,
                max,
                now_ms,
                jitter,
                "boom",
            )
        };
        // retry-after-ms 优先于 retry-after,服务端延迟不做抖动
        assert_eq!(delay(Some("250"), Some("9"), 5, None, 0, 1.0).unwrap(), 250);
        // retry-after 秒数(支持浮点)
        assert_eq!(delay(None, Some("1.5"), 0, None, 0, 0.0).unwrap(), 1500);
        // retry-after HTTP 日期 → 与 now 的差值(上限放宽到 600s)
        let delta = delay(
            None,
            Some("Wed, 21 Oct 2015 07:28:00 GMT"),
            0,
            Some(600_000),
            1_445_412_000_000,
            0.0,
        )
        .unwrap();
        assert_eq!(delta, 480_000);
        // 过去的日期 → 钳到 0 立即重试(now 在日期之后 120s)
        let past = delay(
            None,
            Some("Wed, 21 Oct 2015 07:28:00 GMT"),
            0,
            Some(600_000),
            1_445_412_600_000,
            0.0,
        )
        .unwrap();
        assert_eq!(past, 0);
        // 两个提示都非法 → 指数退避
        assert_eq!(
            delay(Some("abc"), Some("not-a-date"), 0, None, 0, 0.0).unwrap(),
            500
        );
        // 服务端延迟超限 → 立即失败,文案对齐 TS(向上取整秒)
        let error = delay(Some("61000"), None, 0, None, 0, 0.0).unwrap_err();
        assert_eq!(error, "Server requested 61s retry delay (max: 60s). boom");
        // 显式 max_retry_delay_ms 生效
        assert!(delay(Some("3000"), None, 0, Some(2000), 0, 0.0).is_err());
        // max = 0 → 不限制
        assert_eq!(
            delay(Some("999999"), None, 0, Some(0), 0, 0.0).unwrap(),
            999_999
        );
    }

    #[test]
    fn exponential_backoff_caps_and_jitters() {
        assert_eq!(exponential_retry_delay_ms(0, 0.0), 500);
        assert_eq!(exponential_retry_delay_ms(1, 0.0), 1000);
        assert_eq!(exponential_retry_delay_ms(3, 0.0), 4000);
        // 8s 封顶,指数再大也不再增长
        assert_eq!(exponential_retry_delay_ms(5, 0.0), 8000);
        assert_eq!(exponential_retry_delay_ms(30, 0.0), 8000);
        // 抖动把延迟衰减到 75%..100% 区间
        assert_eq!(exponential_retry_delay_ms(0, 1.0), 375);
        assert_eq!(exponential_retry_delay_ms(0, 0.5), 437);
    }

    /// 本地 TCP mock:按脚本逐连接返回响应(读掉请求头与声明的请求体后回写)。
    fn spawn_mock_server(
        script: Vec<String>,
    ) -> (std::net::SocketAddr, std::thread::JoinHandle<u32>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut served = 0u32;
            for response in script {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                served += 1;
                let mut received = Vec::new();
                let mut buffer = [0u8; 1024];
                loop {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..read]);
                    if received.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                // 读完声明的请求体,避免带未读数据关连接触发 RST 吞掉响应
                let headers = String::from_utf8_lossy(&received).to_lowercase();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let body_start = received
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map_or(received.len(), |position| position + 4);
                while received.len() < body_start + content_length {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..read]);
                }
                if stream.write_all(response.as_bytes()).is_err() {
                    break;
                }
                let _ = stream.flush();
                drop(stream);
            }
            served
        });
        (addr, handle)
    }

    fn http_error_response(status_line: &str, body: &str, extra: &[(&str, &str)]) -> String {
        let mut response = format!(
            "HTTP/1.1 {status_line}\r\ncontent-length: {}\r\nconnection: close\r\n",
            body.len()
        );
        for (name, value) in extra {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str("\r\n");
        response.push_str(body);
        response
    }

    fn http_sse_response(data: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{data}",
            data.len()
        )
    }

    /// 收集终态事件(跳过 start)。
    async fn collect_terminal(
        mut stream: AssistantMessageEventStream,
    ) -> Option<AssistantMessageEvent> {
        use futures::StreamExt;
        let mut terminal = None;
        while let Some(event) = stream.next().await {
            if !matches!(event, AssistantMessageEvent::Start { .. }) {
                terminal = Some(event);
            }
        }
        terminal
    }

    #[tokio::test]
    async fn provider_retry_reposts_until_success() {
        use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
        use std::sync::Arc;

        let sse = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\"}}\n\n";
        let (addr, handler) = spawn_mock_server(vec![
            http_error_response("500 Internal Server Error", "boom", &[]),
            http_error_response(
                "429 Too Many Requests",
                "slow down",
                &[("retry-after-ms", "50")],
            ),
            http_sse_response(sse),
        ]);

        let on_response_count = Arc::new(AtomicU32::new(0));
        let final_status = Arc::new(AtomicU16::new(0));
        let count = on_response_count.clone();
        let status = final_status.clone();
        let options = SimpleStreamOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(3),
            on_response: Some(Box::new(move |response| {
                count.fetch_add(1, Ordering::SeqCst);
                status.store(response.status, Ordering::SeqCst);
            })),
            ..Default::default()
        };
        let model = test_model(&format!("http://{addr}"));
        let stream = stream_openai_responses(
            model,
            context_of(vec![user_message("hi")]),
            Some(options),
            None,
        );
        let terminal = collect_terminal(stream).await;

        // 三个不同连接上的请求 = 每次重试重新发请求
        assert_eq!(handler.join().unwrap(), 3);
        match terminal {
            Some(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                message,
            }) => {
                assert_eq!(message.response_id.as_deref(), Some("resp_1"));
            }
            other => panic!("expected done, got {other:?}"),
        }
        // on_response 仅在重试收敛后的最终成功响应上回调一次
        assert_eq!(on_response_count.load(Ordering::SeqCst), 1);
        assert_eq!(final_status.load(Ordering::SeqCst), 200);
    }

    #[tokio::test]
    async fn provider_retry_exhaustion_encodes_last_error() {
        let (addr, handler) = spawn_mock_server(vec![
            http_error_response(
                "408 Request Timeout",
                "timeout one",
                &[("retry-after-ms", "30")],
            ),
            http_error_response(
                "408 Request Timeout",
                "timeout two",
                &[("retry-after-ms", "30")],
            ),
        ]);
        let options = SimpleStreamOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(1),
            ..Default::default()
        };
        let model = test_model(&format!("http://{addr}"));
        let stream = stream_openai_responses(
            model,
            context_of(vec![user_message("hi")]),
            Some(options),
            None,
        );
        let terminal = collect_terminal(stream).await;

        // 首次 + 1 次重试 = 2 个请求,之后耗尽
        assert_eq!(handler.join().unwrap(), 2);
        match terminal {
            Some(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error,
            }) => {
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("OpenAI API error (408): timeout two")
                );
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provider_retry_backoff_is_cancelable() {
        let (addr, handler) = spawn_mock_server(vec![http_error_response(
            "408 Request Timeout",
            "timeout",
            &[("retry-after-ms", "60000")],
        )]);
        let token = CancellationToken::new();
        let options = SimpleStreamOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(2),
            ..Default::default()
        };
        let model = test_model(&format!("http://{addr}"));
        let signal_clone = token.clone();
        let stream = stream_openai_responses(
            model,
            context_of(vec![user_message("hi")]),
            Some(options),
            Some(token),
        );
        // 等首个失败进入 60s 退避后取消 → 立即编码为 aborted,而非等满延迟
        tokio::time::sleep(Duration::from_millis(300)).await;
        signal_clone.cancel();
        let terminal = collect_terminal(stream).await;

        let _ = handler.join();
        match terminal {
            Some(AssistantMessageEvent::Error {
                reason: StopReason::Aborted,
                error,
            }) => {
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => panic!("expected aborted, got {other:?}"),
        }
    }
}
