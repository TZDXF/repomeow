//! OpenAI 兼容 Chat Completions provider:pi-ai `api/openai-completions.ts`(0.84.4)的 Rust 复刻。
//!
//! 组成:
//! - [`build_request_body`]:纯函数,把 [`Model`] + [`Context`] + [`SimpleStreamOptions`]
//!   序列化为 OpenAI 兼容请求体(消息序列化、tools、thinking 参数各分支、compat 探测融合)。
//! - [`stream_openai_completions`]:流式入口,reqwest POST `{base_url}/chat/completions`
//!   走 SSE,把 chunk 聚合为 [`AssistantMessageEvent`] 推入 [`EventStreamWriter`];
//!   失败/中止编码进流(stopReason error/aborted + errorMessage),不 panic、不抛出。
//! - [`send_with_retry`]:provider 内层重试(TS `utils/provider-retry.ts`):
//!   `max_retries` 缺省 2、x-should-retry 头优先、408/409/429/5xx 与传输错误可重试、
//!   retry-after-ms/retry-after 服务端延迟(超 `max_retry_delay_ms` 缺省 60s 立即失败)、
//!   指数退避 + 抖动,退避可被 `CancellationToken` 中断;每次重试都重新发请求。
//! - [`SseDecoder`]:字节流 → SSE 事件的纯解码器,便于单测。
//! - [`StreamAggregator`]:SSE chunk → 事件的纯聚合逻辑,便于单测。
//! - [`crate::agent::llm::validate`] 的消费方(agent-loop)负责工具参数校验,本模块不重复。
//!
//! 与蓝本的已知偏差见模块底部 tests(无 constrained sampling/grammar tools、
//! 无 prompt cache retention、cost 以模型费率计算)。请求路径不走 async-openai:
//! 其错误类型丢弃响应头,无法支撑 provider 重试的 x-should-retry/retry-after 语义。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::event_stream::{event_stream, EventStreamWriter};
use super::types::{
    user_agent, AssistantContent, AssistantMessage, AssistantMessageEvent,
    AssistantMessageEventStream, CacheRetention, Context, InputKind, MaxTokensField, Message,
    Model, ModelThinkingLevel, SimpleStreamOptions, StopReason, TextOrImageContent,
    ThinkingBudgets, ThinkingFormat, ThinkingLevel, ThinkingTokenBudgetField, Tool, ToolCall,
    ToolChoice, ToolResultMessage, Usage, UsageCost, UserContent,
};
use crate::time_util::now_ts_nanos;

const ASSISTANT_BRIDGE_TEXT: &str = "I have processed the tool results.";
const TOOL_RESULT_IMAGE_TEXT: &str = "(see attached image)";
const TOOL_RESULT_EMPTY_TEXT: &str = "(no tool output)";
const TOOL_IMAGE_PLACEHOLDER: &str = "(tool image omitted: model does not support images)";
const USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const ATTACHED_IMAGES_TEXT: &str = "Attached image(s) from tool result:";
const SYNTHETIC_TOOL_RESULT_TEXT: &str = "No result provided";
const OPENAI_HOST_SUFFIX: &str = "api.openai.com";

// ── 客户端与错误 ─────────────────────────────────────────────────────

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

/// HTTP 非 2xx → 错误文案(对齐 async-openai ApiError 的 "{status}: {message}" 形状:
/// 优先取 `{"error":{"message"}}` JSON,否则原文截断;空 body 只留状态码)。
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

// ── compat 解析(TS detectCompat / getCompat) ─────────────────────────

/// 解析后的兼容开关(显式 model.compat 覆盖 URL 探测默认)。
#[derive(Clone, Copy, Debug)]
struct ResolvedCompat {
    supports_store: bool,
    supports_developer_role: bool,
    supports_reasoning_effort: bool,
    supports_usage_in_streaming: bool,
    supports_finish_reason: bool,
    max_tokens_field: MaxTokensField,
    requires_tool_result_name: bool,
    requires_assistant_after_tool_result: bool,
    requires_thinking_as_text: bool,
    requires_reasoning_content_on_assistant_messages: bool,
    thinking_format: ThinkingFormat,
    thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
    supports_strict_mode: bool,
}

/// 按 provider/baseUrl 子串探测(对齐 TS detectCompat 主流分支;额外补
/// dashscope/qwen —— 蓝本未探测该家,应用生态常用,见交付说明偏差列表)。
fn detect_compat(model: &Model) -> ResolvedCompat {
    let provider = model.provider.as_str();
    let base_url = model.base_url.as_str();
    let base_lower = base_url.to_lowercase();

    let is_zai = provider == "zai"
        || provider == "zai-coding-cn"
        || base_url.contains("api.z.ai")
        || base_url.contains("open.bigmodel.cn");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot = provider == "moonshotai"
        || provider == "moonshotai-cn"
        || base_url.contains("api.moonshot.");
    let is_openrouter = provider == "openrouter" || base_url.contains("openrouter.ai");
    let is_cloudflare_workers_ai =
        provider == "cloudflare-workers-ai" || base_url.contains("api.cloudflare.com");
    let is_cloudflare_ai_gateway =
        provider == "cloudflare-ai-gateway" || base_url.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider == "nvidia" || base_url.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider == "ant-ling" || base_url.contains("api.ant-ling.com");
    let is_deepseek = provider == "deepseek" || base_lower.contains("deepseek.com");
    // 扩展分支(蓝本 detectCompat 无):qwen / dashscope
    let is_qwen = provider == "qwen" || provider == "dashscope" || base_lower.contains("dashscope");

    let is_non_standard = is_nvidia
        || provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || is_deepseek
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai")
        || is_cloudflare_workers_ai
        || is_cloudflare_ai_gateway
        || is_ant_ling;

    let use_max_tokens = base_url.contains("chutes.ai")
        || is_deepseek
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai
        || is_qwen;

    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_openrouter_developer_role_model =
        is_openrouter && (model.id.starts_with("anthropic/") || model.id.starts_with("openai/"));

    ResolvedCompat {
        supports_store: !is_non_standard,
        // 对齐蓝本:未知厂商默认支持 developer 角色;不支持的自建网关可
        // 在设置页模型高级配置(model.compat)里显式关闭。
        supports_developer_role: is_openrouter_developer_role_model
            || (!is_non_standard && !is_openrouter && !is_qwen),
        supports_reasoning_effort: !is_grok
            && !is_zai
            && !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia
            && !is_ant_ling
            && !is_qwen,
        supports_usage_in_streaming: true,
        supports_finish_reason: true,
        max_tokens_field: if use_max_tokens {
            MaxTokensField::MaxTokens
        } else {
            MaxTokensField::MaxCompletionTokens
        },
        requires_tool_result_name: false,
        requires_assistant_after_tool_result: false,
        requires_thinking_as_text: false,
        requires_reasoning_content_on_assistant_messages: is_deepseek,
        thinking_format: if is_deepseek {
            ThinkingFormat::Deepseek
        } else if is_zai {
            ThinkingFormat::Zai
        } else if is_together {
            ThinkingFormat::Together
        } else if is_ant_ling {
            ThinkingFormat::AntLing
        } else if is_qwen {
            ThinkingFormat::Qwen
        } else if is_openrouter {
            ThinkingFormat::Openrouter
        } else {
            ThinkingFormat::Openai
        },
        thinking_token_budget_field: None,
        supports_strict_mode: !is_moonshot
            && !is_together
            && !is_cloudflare_ai_gateway
            && !is_nvidia,
    }
}

fn get_compat(model: &Model) -> ResolvedCompat {
    let detected = detect_compat(model);
    let Some(compat) = model.compat.as_ref() else {
        return detected;
    };
    ResolvedCompat {
        supports_store: compat.supports_store.unwrap_or(detected.supports_store),
        supports_developer_role: compat
            .supports_developer_role
            .unwrap_or(detected.supports_developer_role),
        supports_reasoning_effort: compat
            .supports_reasoning_effort
            .unwrap_or(detected.supports_reasoning_effort),
        supports_usage_in_streaming: compat
            .supports_usage_in_streaming
            .unwrap_or(detected.supports_usage_in_streaming),
        supports_finish_reason: compat
            .supports_finish_reason
            .unwrap_or(detected.supports_finish_reason),
        max_tokens_field: compat.max_tokens_field.unwrap_or(detected.max_tokens_field),
        requires_tool_result_name: compat
            .requires_tool_result_name
            .unwrap_or(detected.requires_tool_result_name),
        requires_assistant_after_tool_result: compat
            .requires_assistant_after_tool_result
            .unwrap_or(detected.requires_assistant_after_tool_result),
        requires_thinking_as_text: compat
            .requires_thinking_as_text
            .unwrap_or(detected.requires_thinking_as_text),
        requires_reasoning_content_on_assistant_messages: detected
            .requires_reasoning_content_on_assistant_messages,
        thinking_format: compat.thinking_format.unwrap_or(detected.thinking_format),
        thinking_token_budget_field: compat
            .thinking_token_budget_field
            .or(detected.thinking_token_budget_field),
        supports_strict_mode: compat
            .supports_strict_mode
            .unwrap_or(detected.supports_strict_mode),
    }
}

// ── 消息序列化(TS transformMessages + convertMessages) ───────────────

fn message_timestamp(message: &Message) -> i64 {
    match message {
        Message::User(user) => user.timestamp,
        Message::Assistant(assistant) => assistant.timestamp,
        Message::ToolResult(result) => result.timestamp,
    }
}

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

/// TS shortHash(32 位混淆哈希 ×2,base36 拼接),用于超长 tool call id 归一。
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

/// 管道分隔 id(OpenAI Responses 形状 `{call_id}|{item_id}`)消毒为
/// `^[a-zA-Z0-9_-]+$` 且 ≤40 字符;openai provider 的普通 id 截到 40。
fn normalize_tool_call_id(id: &str, provider: &str) -> String {
    let sanitize = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    if let Some(separator_index) = id.find('|') {
        let call_id = sanitize(&id[..separator_index]);
        let item_id = sanitize(&id[separator_index + 1..]);
        let combined = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        let combined_chars: Vec<char> = combined.chars().collect();
        if combined_chars.len() <= 40 {
            return combined;
        }
        let hash: String = short_hash(id).chars().take(8).collect();
        let prefix_len = 40usize
            .saturating_sub(hash.chars().count())
            .saturating_sub(1)
            .max(1);
        let prefix: String = call_id.chars().take(prefix_len).collect();
        return format!("{prefix}_{hash}");
    }
    if provider == "openai" {
        return id.chars().take(40).collect();
    }
    id.to_string()
}

/// TS transformMessages:非图片模型降级图片;跨模型 replay 清理 thinking/签名与
/// tool call id;孤儿 tool call 合成错误结果;error/aborted assistant 整条丢弃。
fn transform_messages(messages: &[Message], model: &Model) -> Vec<Message> {
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
                                let normalized =
                                    normalize_tool_call_id(&tool_call.id, &model.provider);
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

    // 第二遍:孤儿 tool call 补合成错误结果
    let mut result: Vec<Message> = Vec::new();
    let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
    let mut existing_tool_result_ids: HashSet<String> = HashSet::new();
    // 手动内联的 flush(TS insertSyntheticToolResults)
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

fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|message| match message {
        Message::ToolResult(_) => true,
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_))),
        Message::User(_) => false,
    })
}

fn image_url_part(data: &str, mime_type: &str) -> Value {
    json!({ "type": "image_url", "image_url": { "url": format!("data:{mime_type};base64,{data}") } })
}

/// TS convertMessages:消息 → OpenAI Chat Completions 参数。
fn convert_messages(model: &Model, context: &Context, compat: &ResolvedCompat) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    let transformed = transform_messages(&context.messages, model);

    if let Some(system_prompt) = &context.system_prompt {
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        params.push(json!({ "role": role, "content": system_prompt }));
    }

    let mut last_role: Option<&'static str> = None;
    let mut index = 0usize;
    while index < transformed.len() {
        let message = &transformed[index];

        // 部分提供方不允许 user 紧跟 tool result:插入桥接 assistant
        if compat.requires_assistant_after_tool_result
            && last_role == Some("toolResult")
            && matches!(message, Message::User(_))
        {
            params.push(json!({ "role": "assistant", "content": ASSISTANT_BRIDGE_TEXT }));
        }

        match message {
            Message::User(user) => {
                match &user.content {
                    UserContent::Text(text) => {
                        params.push(json!({ "role": "user", "content": text }));
                        last_role = Some("user");
                    }
                    UserContent::Blocks(blocks) => {
                        let parts: Vec<Value> = blocks
                            .iter()
                            .map(|block| match block {
                                TextOrImageContent::Text { text, .. } => {
                                    json!({ "type": "text", "text": text })
                                }
                                TextOrImageContent::Image { data, mime_type } => {
                                    image_url_part(data, mime_type)
                                }
                            })
                            .collect();
                        if parts.is_empty() {
                            // TS:空内容直接跳过,不更新 lastRole
                            index += 1;
                            continue;
                        }
                        params.push(json!({ "role": "user", "content": parts }));
                        last_role = Some("user");
                    }
                }
                index += 1;
            }
            Message::Assistant(assistant) => {
                if let Some(serialized) = serialize_assistant_message(assistant, model, compat) {
                    params.push(serialized);
                    last_role = Some("assistant");
                }
                index += 1;
            }
            Message::ToolResult(_) => {
                let mut image_parts: Vec<Value> = Vec::new();
                let mut cursor = index;
                while cursor < transformed.len() {
                    let Message::ToolResult(result) = &transformed[cursor] else {
                        break;
                    };
                    let text_result = result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageContent::Text { text, .. } => Some(text.as_str()),
                            TextOrImageContent::Image { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = result
                        .content
                        .iter()
                        .any(|block| matches!(block, TextOrImageContent::Image { .. }));
                    let tool_result_text = if !text_result.is_empty() {
                        text_result
                    } else if has_images {
                        TOOL_RESULT_IMAGE_TEXT.to_string()
                    } else {
                        TOOL_RESULT_EMPTY_TEXT.to_string()
                    };
                    let mut tool_message = Map::new();
                    tool_message.insert("role".to_string(), json!("tool"));
                    tool_message.insert("content".to_string(), json!(tool_result_text));
                    tool_message.insert("tool_call_id".to_string(), json!(result.tool_call_id));
                    if compat.requires_tool_result_name && !result.tool_name.is_empty() {
                        tool_message.insert("name".to_string(), json!(result.tool_name));
                    }
                    params.push(Value::Object(tool_message));

                    if has_images && model.input.contains(&InputKind::Image) {
                        for block in &result.content {
                            if let TextOrImageContent::Image { data, mime_type } = block {
                                image_parts.push(image_url_part(data, mime_type));
                            }
                        }
                    }
                    cursor += 1;
                }
                index = cursor;

                if !image_parts.is_empty() {
                    if compat.requires_assistant_after_tool_result {
                        params
                            .push(json!({ "role": "assistant", "content": ASSISTANT_BRIDGE_TEXT }));
                    }
                    let mut content = vec![json!({ "type": "text", "text": ATTACHED_IMAGES_TEXT })];
                    content.extend(image_parts);
                    params.push(json!({ "role": "user", "content": content }));
                    last_role = Some("user");
                } else {
                    last_role = Some("toolResult");
                }
                // 不做 index += 1:while 直接从第一个非 toolResult 消息继续(TS i = j - 1 + continue)
                continue;
            }
        }
    }

    params
}

/// TS convertMessages assistant 分支:无内容且无 tool_calls 时返回 None(跳过)。
fn serialize_assistant_message(
    assistant: &AssistantMessage,
    model: &Model,
    compat: &ResolvedCompat,
) -> Option<Value> {
    let mut message_obj = Map::new();
    message_obj.insert("role".to_string(), json!("assistant"));
    // 部分提供方不接受 null content;requiresAssistantAfterToolResult 时用空串
    message_obj.insert(
        "content".to_string(),
        if compat.requires_assistant_after_tool_result {
            Value::String(String::new())
        } else {
            Value::Null
        },
    );

    let text_parts: Vec<String> = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        })
        .collect();
    let assistant_text = text_parts.join("");

    let thinking_blocks: Vec<(&str, Option<&str>)> = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Thinking {
                thinking,
                thinking_signature,
                ..
            } => Some((thinking.as_str(), thinking_signature.as_deref())),
            _ => None,
        })
        .collect();
    let tool_calls: Vec<&ToolCall> = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantContent::ToolCall(tool_call) => Some(tool_call),
            _ => None,
        })
        .collect();

    // reasoning_details 回放数据:优先 thinking 签名,退回 toolCall thought_signature
    let signed_details = thinking_blocks
        .iter()
        .find_map(|(_, signature)| parse_openai_reasoning_details(*signature));
    let legacy_details: Vec<Value> = tool_calls
        .iter()
        .filter_map(|tool_call| {
            parse_legacy_encrypted_reasoning_detail(tool_call.thought_signature.as_deref())
        })
        .collect();
    let preserved_details =
        signed_details.or_else(|| (!legacy_details.is_empty()).then_some(legacy_details));

    let non_empty_thinking: Vec<(&str, Option<&str>)> = thinking_blocks
        .iter()
        .copied()
        .filter(|(thinking, _)| !thinking.trim().is_empty())
        .collect();

    if !non_empty_thinking.is_empty() {
        if compat.requires_thinking_as_text {
            // thinking 降级为纯文本(不带标签,避免模型模仿)
            let thinking_text = non_empty_thinking
                .iter()
                .map(|(thinking, _)| (*thinking).to_string())
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut parts = vec![json!({ "type": "text", "text": thinking_text })];
            parts.extend(
                text_parts
                    .iter()
                    .map(|text| json!({ "type": "text", "text": text })),
            );
            message_obj.insert("content".to_string(), Value::Array(parts));
        } else {
            // OpenAI Chat Completions 标准格式:content 用纯字符串
            if !assistant_text.is_empty() {
                message_obj.insert("content".to_string(), Value::String(assistant_text));
            }
            if preserved_details.is_none() {
                let mut signature = non_empty_thinking[0].1;
                if model.provider == "opencode-go" && signature == Some("reasoning") {
                    signature = Some("reasoning_content");
                }
                if let Some(field) = signature {
                    if matches!(field, "reasoning" | "reasoning_content" | "reasoning_text") {
                        let joined = non_empty_thinking
                            .iter()
                            .map(|(thinking, _)| (*thinking).to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        message_obj.insert(field.to_string(), Value::String(joined));
                    }
                }
            }
        }
    } else if !assistant_text.is_empty() {
        message_obj.insert("content".to_string(), Value::String(assistant_text));
    }

    if !tool_calls.is_empty() {
        message_obj.insert(
            "tool_calls".to_string(),
            Value::Array(
                tool_calls
                    .iter()
                    .map(|tool_call| {
                        json!({
                            "id": tool_call.id,
                            "type": "function",
                            "function": {
                                "name": tool_call.name,
                                "arguments": serde_json::to_string(&tool_call.arguments)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            },
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(details) = preserved_details {
        message_obj.insert("reasoning_details".to_string(), Value::Array(details));
    }
    if compat.requires_reasoning_content_on_assistant_messages
        && model.reasoning
        && !message_obj.contains_key("reasoning_content")
    {
        message_obj.insert(
            "reasoning_content".to_string(),
            Value::String(String::new()),
        );
    }

    let has_content = match message_obj.get("content") {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(parts)) => !parts.is_empty(),
        Some(Value::Null) | None => false,
        _ => true,
    };
    let has_tool_calls = message_obj.contains_key("tool_calls");
    if has_content || has_tool_calls {
        Some(Value::Object(message_obj))
    } else {
        None
    }
}

// ── tools ─────────────────────────────────────────────────────────────

/// OpenAI function tools;无 constrained sampling 配置时 strict 恒 false
/// (仅在 supports_strict_mode 时携带该键)。
fn convert_tools(tools: &[Tool], compat: &ResolvedCompat) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut function = Map::new();
            function.insert("name".to_string(), json!(tool.name));
            function.insert("description".to_string(), json!(tool.description));
            function.insert("parameters".to_string(), tool.parameters.clone());
            if compat.supports_strict_mode {
                function.insert("strict".to_string(), json!(false));
            }
            json!({ "type": "function", "function": function })
        })
        .collect()
}

// ── thinking 级别映射与参数 ────────────────────────────────────────────

fn level_key(level: ModelThinkingLevel) -> &'static str {
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
        ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
        ThinkingLevel::Low => ModelThinkingLevel::Low,
        ThinkingLevel::Medium => ModelThinkingLevel::Medium,
        ThinkingLevel::High => ModelThinkingLevel::High,
        ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
        ThinkingLevel::Max => ModelThinkingLevel::Max,
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

/// `mappedEffort === undefined ? level : mappedEffort` + typeof string 语义
/// (null → 不下发)。
fn map_level_strict(model: &Model, key: &str) -> Option<String> {
    match map_level(model, key) {
        MappedLevel::Value(value) => Some(value),
        MappedLevel::Missing => Some(key.to_string()),
        MappedLevel::Null => None,
    }
}

fn supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
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

fn thinking_level_from_model(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
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

const MIN_ANSWER_TOKENS: i64 = 1024;
const DEFAULT_THINKING_BUDGET_MINIMAL: i64 = 1024;
const DEFAULT_THINKING_BUDGET_LOW: i64 = 2048;
const DEFAULT_THINKING_BUDGET_MEDIUM: i64 = 8192;
const DEFAULT_THINKING_BUDGET_HIGH: i64 = 16384;

fn clamp_reasoning_level(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Xhigh | ThinkingLevel::Max => ThinkingLevel::High,
        other => other,
    }
}

fn thinking_budget_for_level(level: ThinkingLevel, custom: Option<&ThinkingBudgets>) -> i64 {
    let level = clamp_reasoning_level(level);
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

/// TS resolveClampedThinkingBudget:effort 存在且模型支持推理时给出顶层预算。
fn resolve_clamped_thinking_budget(
    model: &Model,
    effort: Option<ThinkingLevel>,
    options: Option<&SimpleStreamOptions>,
    ceiling: Option<i64>,
) -> Option<i64> {
    let effort = effort?;
    if !model.reasoning {
        return None;
    }
    let ceiling = ceiling.unwrap_or(model.max_tokens);
    let budget = clamp_thinking_budget_to_answer_room(
        thinking_budget_for_level(
            effort,
            options.and_then(|options| options.thinking_budgets.as_ref()),
        ),
        ceiling,
    );
    (budget > 0).then_some(budget)
}

fn thinking_token_budget_field_key(field: ThinkingTokenBudgetField) -> &'static str {
    match field {
        ThinkingTokenBudgetField::ThinkingTokenBudget => "thinking_token_budget",
        ThinkingTokenBudgetField::ThinkingBudget => "thinking_budget",
        ThinkingTokenBudgetField::ThinkingBudgetTokens => "thinking_budget_tokens",
    }
}

/// thinking 参数各格式分支(对齐 TS buildParams 的 thinkingFormat 链)。
/// 仅在 model.reasoning 时下发任何 thinking 字段(与蓝本一致)。
fn apply_thinking_params(
    body: &mut Map<String, Value>,
    model: &Model,
    compat: &ResolvedCompat,
    effort: Option<ThinkingLevel>,
) {
    if !model.reasoning {
        return;
    }
    let effort_key = effort.map(thinking_level_key);
    match compat.thinking_format {
        ThinkingFormat::Zai => {
            body.insert(
                "thinking".to_string(),
                if effort.is_some() {
                    json!({ "type": "enabled", "clear_thinking": false })
                } else {
                    json!({ "type": "disabled" })
                },
            );
            if effort.is_some() && compat.supports_reasoning_effort {
                if let Some(key) = effort_key {
                    if let Some(value) = map_level_strict(model, key) {
                        body.insert("reasoning_effort".to_string(), json!(value));
                    }
                }
            }
        }
        ThinkingFormat::Qwen => {
            body.insert("enable_thinking".to_string(), json!(effort.is_some()));
            if effort.is_some() && compat.supports_reasoning_effort {
                if let Some(key) = effort_key {
                    body.insert(
                        "reasoning_effort".to_string(),
                        json!(map_level_or_key(model, key)),
                    );
                }
            }
        }
        ThinkingFormat::QwenChatTemplate => {
            body.insert(
                "chat_template_kwargs".to_string(),
                json!({ "enable_thinking": effort.is_some(), "preserve_thinking": true }),
            );
        }
        // chat-template:compat.chat_template_kwargs 未建模(空表)→ 不下发
        ThinkingFormat::ChatTemplate => {}
        ThinkingFormat::Baseten => {
            if compat.supports_reasoning_effort {
                let resolved = match effort_key {
                    Some(key) => map_level_strict(model, key),
                    None => match map_level(model, "off") {
                        MappedLevel::Value(value) => Some(value),
                        _ => None,
                    },
                };
                if let Some(value) = resolved {
                    body.insert("reasoning_effort".to_string(), json!(value));
                }
            }
        }
        ThinkingFormat::Deepseek => {
            if effort.is_some() {
                body.insert("thinking".to_string(), json!({ "type": "enabled" }));
            } else if !matches!(map_level(model, "off"), MappedLevel::Null) {
                body.insert("thinking".to_string(), json!({ "type": "disabled" }));
            }
            if effort.is_some() && compat.supports_reasoning_effort {
                if let Some(key) = effort_key {
                    body.insert(
                        "reasoning_effort".to_string(),
                        json!(map_level_or_key(model, key)),
                    );
                }
            }
        }
        ThinkingFormat::Openrouter => {
            if let Some(key) = effort_key {
                body.insert(
                    "reasoning".to_string(),
                    json!({ "effort": map_level_or_key(model, key) }),
                );
            } else if !matches!(map_level(model, "off"), MappedLevel::Null) {
                let value = match map_level(model, "off") {
                    MappedLevel::Value(value) => value,
                    _ => "none".to_string(),
                };
                body.insert("reasoning".to_string(), json!({ "effort": value }));
            }
        }
        ThinkingFormat::AntLing => {
            if let Some(key) = effort_key {
                if let MappedLevel::Value(value) = map_level(model, key) {
                    body.insert("reasoning".to_string(), json!({ "effort": value }));
                }
            }
        }
        ThinkingFormat::Together => {
            body.insert(
                "reasoning".to_string(),
                json!({ "enabled": effort.is_some() }),
            );
            if effort.is_some() && compat.supports_reasoning_effort {
                if let Some(key) = effort_key {
                    body.insert(
                        "reasoning_effort".to_string(),
                        json!(map_level_or_key(model, key)),
                    );
                }
            }
        }
        ThinkingFormat::StringThinking => {
            if let Some(key) = effort_key {
                body.insert("thinking".to_string(), json!(map_level_or_key(model, key)));
            } else if !matches!(map_level(model, "off"), MappedLevel::Null) {
                let value = match map_level(model, "off") {
                    MappedLevel::Value(value) => value,
                    _ => "none".to_string(),
                };
                body.insert("thinking".to_string(), json!(value));
            }
        }
        ThinkingFormat::Openai => {
            if effort.is_some() {
                if compat.supports_reasoning_effort {
                    if let Some(key) = effort_key {
                        body.insert(
                            "reasoning_effort".to_string(),
                            json!(map_level_or_key(model, key)),
                        );
                    }
                }
            } else if compat.supports_reasoning_effort {
                if let MappedLevel::Value(value) = map_level(model, "off") {
                    body.insert("reasoning_effort".to_string(), json!(value));
                }
            }
        }
    }
}

// ── 上下文预算估算(TS utils/estimate.ts) ─────────────────────────────

const CHARS_PER_TOKEN: i64 = 4;
const ESTIMATED_IMAGE_CHARS: i64 = 4800;
const CONTEXT_SAFETY_TOKENS: i64 = 4096;
const MIN_MAX_TOKENS: i64 = 1;

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

/// TS estimateContextTokens:优先用最近可用 assistant usage,补尾部增量。
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

// ── 请求体构造 ────────────────────────────────────────────────────────

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

/// TS clampOpenAIPromptCacheKey:超过 64 字符按 Unicode 字符截断。
fn clamp_openai_prompt_cache_key(key: &str) -> String {
    const MAX_LENGTH: usize = 64;
    key.chars().take(MAX_LENGTH).collect()
}

/// 纯函数:构造 OpenAI 兼容 streaming 请求体(恒 `stream: true`)。
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> Value {
    let compat = get_compat(model);
    let messages = convert_messages(model, context, &compat);

    let mut body = Map::new();
    body.insert("model".to_string(), json!(model.id));
    body.insert("messages".to_string(), Value::Array(messages));
    body.insert("stream".to_string(), json!(true));

    // OpenAI prompt cache key(官方端点 + 未禁用缓存时)
    let cache_retention = options
        .and_then(|options| options.cache_retention)
        .unwrap_or(CacheRetention::Short);
    if model.base_url.contains(OPENAI_HOST_SUFFIX) && cache_retention != CacheRetention::None {
        if let Some(session_id) = options.and_then(|options| options.session_id.as_deref()) {
            body.insert(
                "prompt_cache_key".to_string(),
                json!(clamp_openai_prompt_cache_key(session_id)),
            );
        }
    }

    if compat.supports_usage_in_streaming {
        body.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }
    if compat.supports_store {
        body.insert("store".to_string(), json!(false));
    }

    // max_tokens:options 优先,回退 model 上限;按上下文预算收敛;
    // 两者均不可用(<= 0)时不下发,交由提供方默认(偏差见交付说明)。
    let mut emitted_max_tokens: Option<i64> = None;
    let requested = options
        .and_then(|options| options.max_tokens)
        .map(i64::from)
        .unwrap_or(model.max_tokens);
    if requested > 0 {
        let clamped = clamp_max_tokens_to_context(model, context, requested);
        if clamped > 0 {
            let field = match compat.max_tokens_field {
                MaxTokensField::MaxTokens => "max_tokens",
                MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
            };
            body.insert(field.to_string(), json!(clamped));
            emitted_max_tokens = Some(clamped);
        }
    }

    if let Some(temperature) = options.and_then(|options| options.temperature) {
        body.insert("temperature".to_string(), json!(temperature));
    }

    let tool_values = convert_tools(&context.tools, &compat);
    if !tool_values.is_empty() {
        body.insert("tools".to_string(), Value::Array(tool_values));
    } else if has_tool_history(&context.messages) {
        // 经代理的 Anthropic 等要求带 tools 参数,即便为空
        body.insert("tools".to_string(), json!([]));
    }

    if let Some(tool_choice) = options.and_then(|options| options.tool_choice) {
        let value = match tool_choice {
            ToolChoice::Auto => "auto",
            ToolChoice::None => "none",
        };
        body.insert("tool_choice".to_string(), json!(value));
    }

    // thinking 参数:options.reasoning 经模型支持级别钳制;off(None)→ 蓝本各 off 分支
    let reasoning_effort = options
        .and_then(|options| options.reasoning)
        .and_then(|level| clamp_thinking_level(model, level));
    apply_thinking_params(&mut body, model, &compat, reasoning_effort);

    // 顶层 thinking token 预算字段(独立于 thinkingFormat)
    if let Some(field) = compat.thinking_token_budget_field {
        if let Some(budget) =
            resolve_clamped_thinking_budget(model, reasoning_effort, options, emitted_max_tokens)
        {
            body.insert(
                thinking_token_budget_field_key(field).to_string(),
                json!(budget),
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

// ── chunk 用量与停止原因 ──────────────────────────────────────────────

/// TS parseChunkUsage + calculateCost:cacheRead 取
/// prompt_tokens_details.cached_tokens → prompt_cache_hit_tokens → cached_tokens;
/// input 扣除缓存命中/写入;reasoning 是 completion 的子集,未上报时缺省。
fn parse_chunk_usage(raw_usage: &Value, model: &Model) -> Usage {
    let number_of = |key: &str| raw_usage.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let prompt_tokens = number_of("prompt_tokens") as i64;
    let details = raw_usage.get("prompt_tokens_details");
    let cache_read_tokens = details
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_i64)
        .or_else(|| {
            raw_usage
                .get("prompt_cache_hit_tokens")
                .and_then(Value::as_i64)
        })
        .or_else(|| raw_usage.get("cached_tokens").and_then(Value::as_i64))
        .unwrap_or(0);
    let cache_write_tokens = details
        .and_then(|details| details.get("cache_write_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let input = (prompt_tokens - cache_read_tokens - cache_write_tokens).max(0);
    let output_tokens = number_of("completion_tokens") as i64;
    let reasoning = raw_usage
        .get("completion_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_i64);

    let mut usage = Usage {
        input,
        output: output_tokens,
        cache_read: cache_read_tokens,
        cache_write: cache_write_tokens,
        cache_write_1h: None,
        reasoning,
        total_tokens: input + output_tokens + cache_read_tokens + cache_write_tokens,
        cost: UsageCost::default(),
    };
    calculate_cost(model, &mut usage);
    usage
}

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

/// TS mapStopReason:finish_reason → 统一 StopReason;未知/内容过滤 → error。
fn map_stop_reason(finish_reason: &str) -> (StopReason, Option<String>) {
    match finish_reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        "content_filter" => (
            StopReason::Error,
            Some("Provider finish_reason: content_filter".to_string()),
        ),
        "network_error" => (
            StopReason::Error,
            Some("Provider finish_reason: network_error".to_string()),
        ),
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

// ── 部分容错 JSON(TS utils/json-parse.ts) ───────────────────────────

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
/// 处理:未闭合字符串(值保留部分文本/键丢弃)、未闭合容器、悬挂 `,`/`:`,
/// 尾部残缺字面量(true/数字残片)。
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
        // 候选 1:保留字符串部分内容后闭合
        let mut kept = input.to_string();
        if escaped {
            kept.pop();
        }
        kept.push('"');
        candidates.push(kept);
        // 候选 2:丢弃整段未完成字符串(键或值)
        candidates.push(input[..string_start].to_string());
    } else {
        // 候选 1:尾部字面量可能已完整
        candidates.push(input.to_string());
        // 候选 2:截掉尾部残缺字面量(true/数字残片等)
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
            // 丢弃悬挂的键:回退到该键的起始引号,可能再次暴露逗号
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

// ── OpenAI reasoning_details 回放数据 ─────────────────────────────────

/// TS isOpenAIReasoningDetail:summary/encrypted/text 三类,公共字段类型受约束。
fn is_openai_reasoning_detail(detail: &Value) -> bool {
    let Some(object) = detail.as_object() else {
        return false;
    };
    let string_or_absent = |key: &str| {
        object
            .get(key)
            .is_none_or(|value| value.is_null() || value.is_string())
    };
    if !string_or_absent("id") || !string_or_absent("signature") {
        return false;
    }
    if object.get("format").is_some_and(|value| !value.is_string()) {
        return false;
    }
    if object.get("index").is_some_and(|value| !value.is_number()) {
        return false;
    }
    match object.get("type").and_then(Value::as_str) {
        Some("reasoning.summary") => object.get("summary").is_some_and(Value::is_string),
        Some("reasoning.encrypted") => object.get("data").is_some_and(Value::is_string),
        Some("reasoning.text") => object.get("text").is_some_and(Value::is_string),
        _ => false,
    }
}

/// TS parseOpenAIReasoningDetails:签名 JSON 解析为合法 detail 数组。
fn parse_openai_reasoning_details(signature: Option<&str>) -> Option<Vec<Value>> {
    let signature = signature?;
    let parsed = serde_json::from_str::<Value>(signature).ok()?;
    let items = parsed.as_array()?;
    if items.is_empty() || !items.iter().all(is_openai_reasoning_detail) {
        return None;
    }
    Some(items.clone())
}

/// TS parseLegacyEncryptedReasoningDetail:单个 encrypted detail(带非空 id/data)。
fn parse_legacy_encrypted_reasoning_detail(signature: Option<&str>) -> Option<Value> {
    let signature = signature?;
    let parsed = serde_json::from_str::<Value>(signature).ok()?;
    let object = parsed.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("reasoning.encrypted") {
        return None;
    }
    let id = object.get("id").and_then(Value::as_str)?;
    let data = object.get("data").and_then(Value::as_str)?;
    if id.is_empty() || data.is_empty() {
        return None;
    }
    Some(parsed)
}

fn value_is_blank(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        _ => false,
    }
}

/// TS appendOpenAIReasoningDetail:相邻同类 text/summary 合并,其余追加;
/// 公共字段按 ??=/||= 语义补齐。
fn append_openai_reasoning_detail(details: &mut Vec<Value>, detail: Value) {
    let kind = detail
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let is_text = kind == "reasoning.text";
    let is_summary = kind == "reasoning.summary";
    let mut merged = false;
    if is_text || is_summary {
        let text_key = if is_text { "text" } else { "summary" };
        if let Some(last) = details.last_mut() {
            if last.get("type").and_then(Value::as_str) == Some(kind.as_str()) {
                if let Some(last_object) = last.as_object_mut() {
                    let delta_text = detail
                        .get(text_key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let previous = last_object
                        .get(text_key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    last_object.insert(
                        text_key.to_string(),
                        Value::String(format!("{previous}{delta_text}")),
                    );
                    // signature ||= detail.signature(空串/null/缺失时覆盖)
                    if value_is_blank(last_object.get("signature")) {
                        if let Some(signature) = detail.get("signature") {
                            last_object.insert("signature".to_string(), signature.clone());
                        }
                    }
                    // id ??= / index ??=(null 视为缺省)
                    if value_is_null_or_absent(last_object.get("id")) {
                        if let Some(id) = detail.get("id") {
                            last_object.insert("id".to_string(), id.clone());
                        }
                    }
                    if value_is_null_or_absent(last_object.get("index")) {
                        if let Some(index) = detail.get("index") {
                            last_object.insert("index".to_string(), index.clone());
                        }
                    }
                    if value_is_null_or_absent(last_object.get("format")) {
                        if let Some(format) = detail.get("format").filter(|value| !value.is_null())
                        {
                            last_object.insert("format".to_string(), format.clone());
                        }
                    }
                    merged = true;
                }
            }
        }
    }
    if !merged {
        details.push(detail);
    }
}

fn value_is_null_or_absent(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

// ── 流聚合器 ──────────────────────────────────────────────────────────

/// SSE chunk → AssistantMessageEvent 的纯聚合逻辑(便于单测)。
/// 事件序列契约:先 `start`(由 [`stream_openai_completions`] 发出),
/// 终止于 `done` 或 `error`;partial 为累积消息快照。
struct StreamAggregator {
    model: Model,
    compat: ResolvedCompat,
    output: AssistantMessage,
    text_open: Option<usize>,
    thinking_open: Option<usize>,
    tool_blocks_by_index: HashMap<i64, usize>,
    tool_blocks_by_id: HashMap<String, usize>,
    partial_args: HashMap<usize, String>,
    has_finish_reason: bool,
    streamed_reasoning_details: Option<Vec<Value>>,
}

impl StreamAggregator {
    fn new(model: &Model, compat: ResolvedCompat) -> Self {
        Self {
            model: model.clone(),
            compat,
            output: new_assistant_message(model),
            text_open: None,
            thinking_open: None,
            tool_blocks_by_index: HashMap::new(),
            tool_blocks_by_id: HashMap::new(),
            partial_args: HashMap::new(),
            has_finish_reason: false,
            streamed_reasoning_details: None,
        }
    }

    fn output(&self) -> &AssistantMessage {
        &self.output
    }

    fn has_tool_call(&self) -> bool {
        self.output
            .content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_)))
    }

    fn ensure_text_block(&mut self) -> (usize, bool) {
        if let Some(index) = self.text_open {
            return (index, false);
        }
        self.output.content.push(AssistantContent::Text {
            text: String::new(),
            text_signature: None,
        });
        let index = self.output.content.len() - 1;
        self.text_open = Some(index);
        (index, true)
    }

    fn ensure_thinking_block(&mut self, signature: &str) -> (usize, bool) {
        if let Some(index) = self.thinking_open {
            return (index, false);
        }
        self.output.content.push(AssistantContent::Thinking {
            thinking: String::new(),
            thinking_signature: Some(signature.to_string()),
            redacted: false,
        });
        let index = self.output.content.len() - 1;
        self.thinking_open = Some(index);
        (index, true)
    }

    fn tool_block_for(&mut self, stream_index: Option<i64>, id: &str) -> (usize, bool) {
        let mut found: Option<usize> = None;
        if let Some(index) = stream_index {
            found = self.tool_blocks_by_index.get(&index).copied();
        }
        if found.is_none() && !id.is_empty() {
            found = self.tool_blocks_by_id.get(id).copied();
        }
        if let Some(index) = found {
            if let Some(stream_index) = stream_index {
                self.tool_blocks_by_index
                    .entry(stream_index)
                    .or_insert(index);
            }
            if !id.is_empty() {
                self.tool_blocks_by_id.insert(id.to_string(), index);
            }
            return (index, false);
        }
        self.output
            .content
            .push(AssistantContent::ToolCall(ToolCall {
                id: id.to_string(),
                name: String::new(),
                arguments: Map::new(),
                thought_signature: None,
                namespace: None,
            }));
        let index = self.output.content.len() - 1;
        if let Some(stream_index) = stream_index {
            self.tool_blocks_by_index.insert(stream_index, index);
        }
        if !id.is_empty() {
            self.tool_blocks_by_id.insert(id.to_string(), index);
        }
        (index, true)
    }

    fn set_tool_id_if_empty(&mut self, index: usize, id: &str) {
        if let Some(AssistantContent::ToolCall(tool_call)) = self.output.content.get_mut(index) {
            if tool_call.id.is_empty() {
                tool_call.id = id.to_string();
            }
        }
        if !id.is_empty() {
            self.tool_blocks_by_id.insert(id.to_string(), index);
        }
    }

    fn set_tool_name_if_empty(&mut self, index: usize, name: &str) {
        if let Some(AssistantContent::ToolCall(tool_call)) = self.output.content.get_mut(index) {
            if tool_call.name.is_empty() {
                tool_call.name = name.to_string();
            }
        }
    }

    fn append_text(&mut self, index: usize, delta: &str) {
        if let Some(AssistantContent::Text { text, .. }) = self.output.content.get_mut(index) {
            text.push_str(delta);
        }
    }

    fn append_thinking(&mut self, index: usize, delta: &str) {
        if let Some(AssistantContent::Thinking { thinking, .. }) =
            self.output.content.get_mut(index)
        {
            thinking.push_str(delta);
        }
    }

    fn append_tool_args(&mut self, index: usize, delta: &str) {
        let parsed = {
            let entry = self.partial_args.entry(index).or_default();
            entry.push_str(delta);
            parse_streaming_json_object(entry)
        };
        if let Some(AssistantContent::ToolCall(tool_call)) = self.output.content.get_mut(index) {
            tool_call.arguments = parsed;
        }
    }

    /// 应用流内 reasoning_details 到 thinking 块签名(序列化一次)。
    fn apply_streamed_reasoning_details(&mut self) {
        if let Some(details) = self.streamed_reasoning_details.take() {
            let signature = Value::Array(details).to_string();
            if let Some(index) = self.thinking_open {
                if let Some(AssistantContent::Thinking {
                    thinking_signature, ..
                }) = self.output.content.get_mut(index)
                {
                    *thinking_signature = Some(signature);
                }
            }
        }
    }

    /// 消费一个 SSE chunk,返回由此产生的事件(顺序对齐 TS)。
    fn apply_chunk(&mut self, chunk: &Value) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        let Some(chunk_object) = chunk.as_object() else {
            return events;
        };

        if self.output.response_id.is_none() {
            if let Some(id) = chunk_object.get("id").and_then(Value::as_str) {
                self.output.response_id = Some(id.to_string());
            }
        }
        if self.output.response_model.is_none() {
            if let Some(model) = chunk_object.get("model").and_then(Value::as_str) {
                if !model.is_empty() && model != self.model.id {
                    self.output.response_model = Some(model.to_string());
                }
            }
        }

        // usage:chunk 顶层优先;缺失时回退 choice.usage(如 Moonshot)
        let mut usage_taken = false;
        if let Some(usage) = chunk_object.get("usage") {
            if usage.is_object() {
                self.output.usage = parse_chunk_usage(usage, &self.model);
                usage_taken = true;
            }
        }
        if !usage_taken {
            if let Some(choice) = chunk_object
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|c| c.first())
            {
                if let Some(usage) = choice.get("usage").filter(|usage| usage.is_object()) {
                    self.output.usage = parse_chunk_usage(usage, &self.model);
                }
            }
        }

        let Some(choice) = chunk_object
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(Value::as_object)
        else {
            return events;
        };

        if let Some(finish_reason) = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
        {
            self.output.raw_stop_reason = Some(finish_reason.to_string());
            let (stop_reason, error_message) = map_stop_reason(finish_reason);
            self.output.stop_reason = stop_reason;
            if let Some(message) = error_message {
                self.output.error_message = Some(message);
            }
            self.has_finish_reason = true;
        }

        let Some(delta) = choice.get("delta") else {
            return events;
        };

        // 正文增量
        if let Some(content) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
        {
            let (index, created) = self.ensure_text_block();
            if created {
                events.push(AssistantMessageEvent::TextStart {
                    content_index: index as u32,
                    partial: self.output.clone(),
                });
            }
            self.append_text(index, content);
            events.push(AssistantMessageEvent::TextDelta {
                content_index: index as u32,
                delta: content.to_string(),
                partial: self.output.clone(),
            });
        }

        // 思考增量:reasoning_content(llama.cpp)/ reasoning / reasoning_text,
        // 取第一个非空字段避免重复(chutes.ai 两者同文)
        for field in ["reasoning_content", "reasoning", "reasoning_text"] {
            if let Some(value) = delta
                .get(field)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                let signature = if self.model.provider == "opencode-go" && field == "reasoning" {
                    "reasoning_content"
                } else {
                    field
                };
                let (index, created) = self.ensure_thinking_block(signature);
                if created {
                    events.push(AssistantMessageEvent::ThinkingStart {
                        content_index: index as u32,
                        partial: self.output.clone(),
                    });
                }
                self.append_thinking(index, value);
                events.push(AssistantMessageEvent::ThinkingDelta {
                    content_index: index as u32,
                    delta: value.to_string(),
                    partial: self.output.clone(),
                });
                break;
            }
        }

        // 工具调用:按 index/id 聚合 id/name/arguments(字符串拼接后容错解析)
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                let stream_index = tool_call.get("index").and_then(Value::as_i64);
                let id = tool_call.get("id").and_then(Value::as_str).unwrap_or("");
                let name = tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments_delta = tool_call
                    .get("function")
                    .and_then(|function| function.get("arguments"))
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let (index, created) = self.tool_block_for(stream_index, id);
                if created {
                    events.push(AssistantMessageEvent::ToolcallStart {
                        content_index: index as u32,
                        partial: self.output.clone(),
                    });
                }
                if !id.is_empty() {
                    self.set_tool_id_if_empty(index, id);
                }
                if !name.is_empty() {
                    self.set_tool_name_if_empty(index, name);
                }
                let mut delta_text = String::new();
                if !arguments_delta.is_empty() {
                    delta_text = arguments_delta.to_string();
                    self.append_tool_args(index, arguments_delta);
                }
                events.push(AssistantMessageEvent::ToolcallDelta {
                    content_index: index as u32,
                    delta: delta_text,
                    partial: self.output.clone(),
                });
            }
        }

        // OpenRouter reasoning_details(回放元数据,暂存,块结束时序列化)
        if let Some(details) = delta.get("reasoning_details").and_then(Value::as_array) {
            for detail in details {
                if !is_openai_reasoning_detail(detail) {
                    continue;
                }
                let (index, created) = self.ensure_thinking_block("");
                if created {
                    events.push(AssistantMessageEvent::ThinkingStart {
                        content_index: index as u32,
                        partial: self.output.clone(),
                    });
                }
                let streamed = self.streamed_reasoning_details.get_or_insert_with(Vec::new);
                append_openai_reasoning_detail(streamed, detail.clone());
            }
        }

        events
    }

    /// 收尾:为每个内容块补发 end 事件(对齐 TS finishBlock)。
    fn finish_blocks(&mut self) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        self.apply_streamed_reasoning_details();
        for index in 0..self.output.content.len() {
            match self.output.content[index].clone() {
                AssistantContent::Text { text, .. } => {
                    events.push(AssistantMessageEvent::TextEnd {
                        content_index: index as u32,
                        content: text,
                        partial: self.output.clone(),
                    });
                }
                AssistantContent::Thinking { thinking, .. } => {
                    events.push(AssistantMessageEvent::ThinkingEnd {
                        content_index: index as u32,
                        content: thinking,
                        partial: self.output.clone(),
                    });
                }
                AssistantContent::ToolCall(mut tool_call) => {
                    if let Some(raw) = self.partial_args.remove(&index) {
                        tool_call.arguments = parse_streaming_json_object(&raw);
                        self.output.content[index] = AssistantContent::ToolCall(tool_call.clone());
                    }
                    events.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: index as u32,
                        tool_call,
                        partial: self.output.clone(),
                    });
                }
            }
        }
        events
    }

    /// 错误收尾(不补发块 end 事件,对齐 TS catch 分支)。
    fn prepare_error(&mut self, reason: StopReason, message: String) {
        if self.streamed_reasoning_details.is_some() {
            self.apply_streamed_reasoning_details();
        }
        self.output.stop_reason = reason;
        self.output.error_message = Some(message);
    }

    fn error_event(&self, reason: StopReason) -> AssistantMessageEvent {
        AssistantMessageEvent::Error {
            reason,
            error: self.output.clone(),
        }
    }

    /// 请求未建立(取消/HTTP 失败)时的错误收尾;错误路径不补发块 end 事件。
    fn error_final(
        mut self,
        reason: StopReason,
        message: String,
    ) -> (AssistantMessageEvent, AssistantMessage) {
        self.prepare_error(reason, message);
        let event = self.error_event(reason);
        (event, self.output)
    }

    /// 终态判定(对齐 TS stream 尾部逻辑):
    /// 流错误 → error;中止 → error(aborted);无 finish_reason → 兜底/报错;
    /// 其余 → done。返回 (块 end 事件, 最终消息, 终止事件)。
    fn finalize(
        mut self,
        aborted: bool,
        stream_error: Option<String>,
    ) -> (
        Vec<AssistantMessageEvent>,
        AssistantMessage,
        AssistantMessageEvent,
    ) {
        if let Some(message) = stream_error {
            let reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            self.prepare_error(reason, message);
            let event = self.error_event(reason);
            return (Vec::new(), self.output, event);
        }

        let events = self.finish_blocks();

        if aborted {
            self.prepare_error(StopReason::Aborted, "Request was aborted".to_string());
            let event = self.error_event(StopReason::Aborted);
            return (events, self.output, event);
        }

        if !self.has_finish_reason && !self.compat.supports_finish_reason {
            self.output.stop_reason = if self.has_tool_call() {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            };
        }
        if self.output.stop_reason == StopReason::Error {
            let message = self
                .output
                .error_message
                .clone()
                .unwrap_or_else(|| "Provider returned an error stop reason".to_string());
            self.prepare_error(StopReason::Error, message);
            let event = self.error_event(StopReason::Error);
            return (events, self.output, event);
        }
        if (self.compat.supports_finish_reason && !self.has_finish_reason)
            || self.output.stop_reason == StopReason::Pending
        {
            self.prepare_error(
                StopReason::Error,
                "Stream ended without finish_reason".to_string(),
            );
            let event = self.error_event(StopReason::Error);
            return (events, self.output, event);
        }

        let reason = self.output.stop_reason;
        let event = AssistantMessageEvent::Done {
            reason,
            message: self.output.clone(),
        };
        (events, self.output, event)
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

// ── Provider 内层重试(TS utils/provider-retry.ts) ────────────────────

/// TS retryProviderRequest 裸默认为 0;应用侧对齐任务规格:未显式配置时重试 2 次。
const DEFAULT_MAX_RETRIES: u32 = 2;
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// 单次请求失败:错误文案 + 重试判定所需的服务端提示。
struct RequestFailure {
    message: String,
    retryable: bool,
    /// `retry-after-ms` 原始值(毫秒浮点)。
    retry_after_ms: Option<String>,
    /// `retry-after` 原始值(秒数或 HTTP 日期)。
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
/// 全新请求;`options.max_retries` 缺省 2,可重试错误见 [`is_retryable_provider_error`];
/// 服务端延迟超过 `max_retry_delay_ms` 立即失败;退避睡眠可被 `signal` 中断
/// (中断/发送途中取消 → ("Request was aborted", true))。返回 Err 的 bool = 是否因取消而失败。
async fn send_with_retry(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
    body: &Value,
    signal: Option<&CancellationToken>,
) -> Result<reqwest::Response, (String, bool)> {
    let api_key = resolve_api_key(model, options).map_err(|message| (message, false))?;
    let max_retries = options
        .and_then(|options| options.max_retries)
        .unwrap_or(DEFAULT_MAX_RETRIES);
    let max_retry_delay_ms = options.and_then(|options| options.max_retry_delay_ms);
    let mut retries_remaining = max_retries;
    let mut retry_index: u32 = 0;

    loop {
        match send_completions_request(model, options, body, &api_key).await {
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

/// SDK 行为:POST `{base_url}/chat/completions`,返回流式响应。
/// 非 2xx 时读取响应头/响应体格式化为 [`RequestFailure`](重试判定 + 服务端延迟提示);
/// 传输层错误按无 status 处理 = 可重试、无延迟提示。
async fn send_completions_request(
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
    // model.headers / options.headers 最后合并,可覆盖默认(options 优先)
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
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));
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

// ── SSE 解码 ──────────────────────────────────────────────────────────

/// 单条 SSE 事件(eventsource-stream 0.2.3 的观测子集;缺省事件类型 = "message")。
#[derive(Debug)]
struct ServerSentEvent {
    event: Option<String>,
    data: String,
}

/// 字节流 → SSE 事件:行按 `\r\n` / `\r` / `\n` 切分(跨块断行、跨块 UTF-8 安全),
/// `:` 注释行忽略,`event`/`data` 字段累积(值省略冒号 = 空串,剥一个前导空格),
/// 空行分发事件;data 缓冲为空时不分发,流结束时未终止的残缺事件按规范丢弃。
#[derive(Default)]
struct SseDecoder {
    event: Option<String>,
    data: Vec<String>,
    buffer: Vec<u8>,
    started: bool,
}

impl SseDecoder {
    /// 喂入一块字节,返回其中凑齐的事件(可能 0 个或多个)。
    fn push_bytes(&mut self, chunk: &[u8]) -> Vec<ServerSentEvent> {
        self.buffer.extend_from_slice(chunk);
        // 流起始的 UTF-8 BOM 按规范剥离
        if !self.started {
            self.started = true;
            if self.buffer.starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.buffer.drain(..3);
            }
        }
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

    /// 空行分发;":" 注释行忽略;event/data 字段累积。
    fn decode_line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            return self.flush();
        }
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

    /// 分发暂存事件;无 data 行时丢弃(含暂存的 event 类型,对齐 eventsource 的
    /// builder 重置语义),否则逐行 data 以 `\n` 合并。
    fn flush(&mut self) -> Option<ServerSentEvent> {
        let event = self.event.take();
        if self.data.is_empty() {
            return None;
        }
        Some(ServerSentEvent {
            event,
            data: std::mem::take(&mut self.data).join("\n"),
        })
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

/// SSE 事件载荷 → chunk 的消费语义(data "[DONE]" 结束流;event "keepalive" 跳过;
/// 其余 data 必须是 JSON chunk,解析失败 = 流错误)。
#[derive(Debug)]
enum SsePayload {
    Chunk(Value),
    Skip,
    Done,
    Fatal(String),
}

fn decode_chunk_payload(sse: &ServerSentEvent) -> SsePayload {
    if sse.data == "[DONE]" {
        return SsePayload::Done;
    }
    if sse.event.as_deref() == Some("keepalive") {
        return SsePayload::Skip;
    }
    match serde_json::from_str::<Value>(&sse.data) {
        Ok(value) => SsePayload::Chunk(value),
        Err(error) => SsePayload::Fatal(format!(
            "failed to deserialize api response: error:{error} content:{}",
            sse.data
        )),
    }
}

// ── 流式入口 ──────────────────────────────────────────────────────────

/// OpenAI 兼容流式生成:返回事件流(先 `start`,终止于 `done`/`error`)。
/// 失败/中止编码为 stopReason error/aborted 的最终消息,不 panic;
/// `signal` 取消即时生效(连接期与读取期)。
pub fn stream_openai_completions(
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
    let compat = get_compat(&model);
    let mut aggregator = StreamAggregator::new(&model, compat);
    writer.push(AssistantMessageEvent::Start {
        partial: aggregator.output().clone(),
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

    // Provider 内层重试:每次重试重新发请求;取消可打断退避睡眠
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

    let mut decoder = SseDecoder::default();
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
                for sse in decoder.push_bytes(&chunk) {
                    match decode_chunk_payload(&sse) {
                        SsePayload::Chunk(value) => {
                            for event in aggregator.apply_chunk(&value) {
                                writer.push(event);
                            }
                        }
                        SsePayload::Skip => {}
                        // data [DONE]:正常结束(对齐 async-openai 在该哨兵处终止流)
                        SsePayload::Done => break 'read,
                        SsePayload::Fatal(message) => {
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

    let (events, message, terminal) = aggregator.finalize(aborted, stream_error);
    for event in events {
        writer.push(event);
    }
    writer.push(terminal);
    writer.end(message);
}

#[cfg(test)]
mod tests {
    use super::*;
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
            api: API_OPENAI_COMPLETIONS.to_string(),
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

    use super::super::types::{OpenAICompletionsCompat, UserMessage, API_OPENAI_COMPLETIONS};

    fn context_of(messages: Vec<Message>) -> Context {
        Context {
            system_prompt: None,
            messages,
            tools: Vec::new(),
        }
    }

    // ── build_request_body ───────────────────────────────────────────

    #[test]
    fn basic_body_shape_and_system_role() {
        let model = test_model("https://api.openai.com/v1");
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());
        let body = build_request_body(&model, &context, None);

        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be brief");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "hi");
        // api.openai.com 探测:非 non-standard → supportsStore → store:false
        assert_eq!(body["store"], false);
        assert_eq!(body["max_completion_tokens"], serde_json::Value::Null);
        // 无工具且无 tool 历史:不下发 tools
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn developer_role_requires_reasoning_and_compat() {
        let mut model = reasoning_model("https://api.openai.com/v1");
        let mut context = context_of(vec![user_message("hi")]);
        context.system_prompt = Some("be brief".to_string());
        // 默认探测(supportsDeveloperRole = true)但 model.reasoning = true → developer
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["messages"][0]["role"], "developer");

        // 未知厂商(自建网关)同样默认放行,compat 显式关闭后回退 system
        let mut unknown = reasoning_model("http://192.168.3.3:8084/v1");
        let body = build_request_body(&unknown, &context, None);
        assert_eq!(body["messages"][0]["role"], "developer");
        unknown.compat = Some(OpenAICompletionsCompat {
            supports_developer_role: Some(false),
            ..Default::default()
        });
        let body = build_request_body(&unknown, &context, None);
        assert_eq!(body["messages"][0]["role"], "system");

        // 显式关闭 developer role
        model.compat = Some(OpenAICompletionsCompat {
            supports_developer_role: Some(false),
            ..Default::default()
        });
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["messages"][0]["role"], "system");

        // 非 reasoning 模型即便 compat 允许也用 system
        let mut plain = test_model("https://api.openai.com/v1");
        plain.compat = Some(OpenAICompletionsCompat {
            supports_developer_role: Some(true),
            ..Default::default()
        });
        let body = build_request_body(&plain, &context, None);
        assert_eq!(body["messages"][0]["role"], "system");
    }

    #[test]
    fn tool_call_and_result_serialization() {
        let model = test_model("https://api.openai.com/v1");
        let messages = vec![
            user_message("weather?"),
            assistant_message(
                vec![tool_call("call_1", "get_weather", json!({"city": "Oslo"}))],
                StopReason::ToolUse,
            ),
            tool_result_message("call_1", "get_weather", "18C"),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["role"], "assistant");
        assert!(msgs[1]["content"].is_null());
        assert_eq!(msgs[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(msgs[1]["tool_calls"][0]["type"], "function");
        assert_eq!(msgs[1]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(
            msgs[1]["tool_calls"][0]["function"]["arguments"],
            r#"{"city":"Oslo"}"#
        );
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["tool_call_id"], "call_1");
        assert_eq!(msgs[2]["content"], "18C");
        // 带 tool 历史:即便 tools 为空也补 tools: []
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn requires_tool_result_name_and_assistant_bridge() {
        let mut model = test_model("https://example.invalid/v1");
        model.compat = Some(OpenAICompletionsCompat {
            requires_tool_result_name: Some(true),
            requires_assistant_after_tool_result: Some(true),
            ..Default::default()
        });
        let messages = vec![
            user_message("weather?"),
            assistant_message(
                vec![tool_call("call_1", "get_weather", json!({}))],
                StopReason::ToolUse,
            ),
            tool_result_message("call_1", "get_weather", "18C"),
            user_message("thanks"),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["name"], "get_weather");
        assert_eq!(msgs[2]["tool_call_id"], "call_1");
        // assistant content 用空串 + user 前插桥接
        assert_eq!(msgs[1]["content"], "");
        assert_eq!(msgs[3]["role"], "assistant");
        assert_eq!(msgs[3]["content"], ASSISTANT_BRIDGE_TEXT);
        assert_eq!(msgs[4]["role"], "user");
    }

    #[test]
    fn thinking_blocks_as_reasoning_field_and_text_downgrade() {
        let mut model = reasoning_model("https://api.openai.com/v1");
        let thinking = AssistantContent::Thinking {
            thinking: "pondering".to_string(),
            thinking_signature: Some("reasoning_content".to_string()),
            redacted: false,
        };
        let messages = vec![
            user_message("q"),
            assistant_message(
                vec![thinking, AssistantContent::text("answer")],
                StopReason::Stop,
            ),
        ];

        // 默认:thinking 进 reasoning_content 字段,content 为纯文本
        let body = build_request_body(&model, &context_of(messages.clone()), None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"], "answer");
        assert_eq!(msgs[1]["reasoning_content"], "pondering");

        // requires_thinking_as_text:降级为 <文本> 前置内容块
        model.compat = Some(OpenAICompletionsCompat {
            requires_thinking_as_text: Some(true),
            ..Default::default()
        });
        let body = build_request_body(&model, &context_of(messages), None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!(msgs[1]["content"][0]["text"], "pondering");
        assert_eq!(msgs[1]["content"][1]["text"], "answer");
    }

    #[test]
    fn thinking_formats_each_branch() {
        let effort = Some(SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        });
        let context = context_of(vec![user_message("hi")]);

        let make_body = |base_url: &str, compat: OpenAICompletionsCompat| {
            let mut model = reasoning_model(base_url);
            model.compat = Some(compat);
            build_request_body(&model, &context, effort.as_ref())
        };

        let body = make_body(
            "https://api.openai.com/v1",
            OpenAICompletionsCompat::default(),
        );
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("thinking").is_none());

        let body = make_body(
            "https://openrouter.ai/api/v1",
            OpenAICompletionsCompat::default(),
        );
        assert_eq!(body["reasoning"]["effort"], "high");

        let body = make_body(
            "https://api.deepseek.com/v1",
            OpenAICompletionsCompat::default(),
        );
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");

        // deepseek off(无 options):thinking disabled
        let mut model = reasoning_model("https://api.deepseek.com/v1");
        model.compat = Some(OpenAICompletionsCompat::default());
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["thinking"]["type"], "disabled");

        let body = make_body(
            "https://api.z.ai/api/paas/v4",
            OpenAICompletionsCompat::default(),
        );
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["clear_thinking"], false);
        // zai 探测默认不支持 reasoning_effort
        assert!(body.get("reasoning_effort").is_none());

        let body = make_body(
            "https://open.bigmodel.cn/api/paas/v4",
            OpenAICompletionsCompat::default(),
        );
        assert_eq!(body["thinking"]["type"], "enabled");

        let body = make_body(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            OpenAICompletionsCompat::default(),
        );
        assert_eq!(body["enable_thinking"], true);

        let body = make_body(
            "https://api.openai.com/v1",
            OpenAICompletionsCompat {
                thinking_format: Some(ThinkingFormat::QwenChatTemplate),
                ..Default::default()
            },
        );
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(body["chat_template_kwargs"]["preserve_thinking"], true);

        let body = make_body(
            "https://api.openai.com/v1",
            OpenAICompletionsCompat {
                thinking_format: Some(ThinkingFormat::Together),
                ..Default::default()
            },
        );
        assert_eq!(body["reasoning"]["enabled"], true);
        assert_eq!(body["reasoning_effort"], "high");

        let body = make_body(
            "https://api.openai.com/v1",
            OpenAICompletionsCompat {
                thinking_format: Some(ThinkingFormat::StringThinking),
                ..Default::default()
            },
        );
        assert_eq!(body["thinking"], "high");

        // ant-ling:仅当 thinkingLevelMap 有映射才下发
        let body = make_body(
            "https://api.openai.com/v1",
            OpenAICompletionsCompat {
                thinking_format: Some(ThinkingFormat::AntLing),
                ..Default::default()
            },
        );
        assert!(body.get("reasoning").is_none());
        let mut model = reasoning_model("https://api.openai.com/v1");
        model.compat = Some(OpenAICompletionsCompat {
            thinking_format: Some(ThinkingFormat::AntLing),
            ..Default::default()
        });
        let mut map = HashMap::new();
        map.insert("high".to_string(), Some("medium".to_string()));
        model.thinking_level_map = Some(map);
        let body = build_request_body(&model, &context, effort.as_ref());
        assert_eq!(body["reasoning"]["effort"], "medium");
    }

    #[test]
    fn thinking_level_map_overrides_effort() {
        let mut model = reasoning_model("https://api.openai.com/v1");
        let mut map: HashMap<String, Option<String>> = HashMap::new();
        map.insert("high".to_string(), Some("medium".to_string()));
        map.insert("off".to_string(), Some("low".to_string()));
        model.thinking_level_map = Some(map);
        let context = context_of(vec![user_message("hi")]);

        let effort = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&effort));
        assert_eq!(body["reasoning_effort"], "medium");

        // off:openai 格式取 map.off
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["reasoning_effort"], "low");
    }

    #[test]
    fn max_tokens_field_and_context_clamp() {
        let mut model = test_model("https://api.openai.com/v1");
        model.max_tokens = 100_000;
        let context = context_of(vec![user_message("hi")]);
        let options = SimpleStreamOptions {
            max_tokens: Some(100),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["max_completion_tokens"], 100);

        model.compat = Some(OpenAICompletionsCompat {
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            ..Default::default()
        });
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["max_tokens"], 100);
        assert!(body.get("max_completion_tokens").is_none());

        // 无可用上限(from_settings 默认 0)→ 不下发 max_tokens 字段
        let model = test_model("https://api.openai.com/v1");
        let body = build_request_body(&model, &context, None);
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn sampling_params_override_named_fields() {
        let mut model = test_model("https://api.openai.com/v1");
        let mut model_params = HashMap::new();
        model_params.insert("top_p".to_string(), json!(0.9));
        model.sampling_params = Some(model_params);

        let mut options = SimpleStreamOptions::default();
        let mut option_params = HashMap::new();
        option_params.insert("top_p".to_string(), json!(0.5));
        option_params.insert("frequency_penalty".to_string(), json!(1));
        options.sampling_params = Some(option_params);

        let context = context_of(vec![user_message("hi")]);
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["top_p"], 0.5);
        assert_eq!(body["frequency_penalty"], 1);

        // 仅 model 级参数
        let body = build_request_body(&model, &context, None);
        assert_eq!(body["top_p"], 0.9);
    }

    #[test]
    fn tools_serialized_with_strict_flag() {
        let model = test_model("https://api.openai.com/v1");
        let mut context = context_of(vec![user_message("hi")]);
        context.tools = vec![Tool {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({"type": "object", "properties": {}}),
        }];
        let body = build_request_body(&model, &context, None);
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "get_weather");
        assert_eq!(tool["function"]["description"], "Get weather");
        assert_eq!(tool["function"]["strict"], false);

        // 不支持 strict mode:不带 strict 键
        let mut model = test_model("https://api.moonshot.cn/v1");
        model.compat = Some(OpenAICompletionsCompat {
            supports_strict_mode: Some(false),
            ..Default::default()
        });
        let body = build_request_body(&model, &context, None);
        assert!(body["tools"][0]["function"].get("strict").is_none());
    }

    #[test]
    fn tool_choice_and_prompt_cache_key() {
        let model = test_model("https://api.openai.com/v1");
        let context = context_of(vec![user_message("hi")]);
        let options = SimpleStreamOptions {
            tool_choice: Some(ToolChoice::None),
            session_id: Some("sess-123".to_string()),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["tool_choice"], "none");
        assert_eq!(body["prompt_cache_key"], "sess-123");

        // 非官方端点不带 prompt_cache_key
        let model = test_model("https://example.invalid/v1");
        let body = build_request_body(&model, &context, Some(&options));
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn thinking_token_budget_field_emitted() {
        let mut model = reasoning_model("https://api.openai.com/v1");
        model.max_tokens = 8192;
        model.compat = Some(OpenAICompletionsCompat {
            thinking_token_budget_field: Some(ThinkingTokenBudgetField::ThinkingBudget),
            ..Default::default()
        });
        let context = context_of(vec![user_message("hi")]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, Some(&options));
        // 默认 high 预算 16384,被上限 8192-1024 收敛
        assert_eq!(body["thinking_budget"], 7168);
        // 自定义预算覆盖
        let mut options = options;
        options.thinking_budgets = Some(ThinkingBudgets {
            high: Some(2048),
            ..Default::default()
        });
        let body = build_request_body(&model, &context, Some(&options));
        assert_eq!(body["thinking_budget"], 2048);
    }

    #[test]
    fn errored_and_empty_assistant_messages_are_skipped() {
        let model = test_model("https://api.openai.com/v1");
        let messages = vec![
            user_message("hi"),
            assistant_message(vec![], StopReason::Error),
            assistant_message(vec![], StopReason::Stop),
            assistant_message(vec![AssistantContent::text("ok")], StopReason::Stop),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["content"], "ok");
    }

    #[test]
    fn orphan_tool_call_gets_synthetic_result() {
        let model = test_model("https://api.openai.com/v1");
        let messages = vec![
            user_message("hi"),
            assistant_message(
                vec![tool_call("call_9", "get_weather", json!({}))],
                StopReason::ToolUse,
            ),
        ];
        let body = build_request_body(&model, &context_of(messages), None);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2]["role"], "tool");
        assert_eq!(msgs[2]["tool_call_id"], "call_9");
        assert_eq!(msgs[2]["content"], SYNTHETIC_TOOL_RESULT_TEXT);
    }

    #[test]
    fn piped_tool_call_ids_are_normalized_consistently() {
        let model = test_model("https://api.openai.com/v1");
        let piped = "call_abc|resp_1234567890+abcdefghij";
        let mut cross_model = assistant_message(
            vec![tool_call(piped, "get_weather", json!({}))],
            StopReason::ToolUse,
        );
        // 跨模型 replay 才触发 id 归一(对齐 TS isSameModel 条件)
        if let Message::Assistant(assistant) = &mut cross_model {
            assistant.model = "other-model".to_string();
        }
        let messages = vec![cross_model, tool_result_message(piped, "get_weather", "ok")];
        let body = build_request_body(&model, &context_of(messages), None);
        let msgs = body["messages"].as_array().unwrap();
        let call_id = msgs[0]["tool_calls"][0]["id"].as_str().unwrap();
        let result_id = msgs[1]["tool_call_id"].as_str().unwrap();
        assert_eq!(call_id, result_id);
        assert!(call_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        assert!(call_id.chars().count() <= 40);
    }

    #[test]
    fn image_blocks_become_data_urls() {
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
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,QUJD");
    }

    #[test]
    fn compat_detection_by_base_url() {
        let cases: &[(&str, ThinkingFormat, MaxTokensField)] = &[
            (
                "https://openrouter.ai/api/v1",
                ThinkingFormat::Openrouter,
                MaxTokensField::MaxCompletionTokens,
            ),
            (
                "https://api.deepseek.com/v1",
                ThinkingFormat::Deepseek,
                MaxTokensField::MaxTokens,
            ),
            (
                "https://api.z.ai/api/paas/v4",
                ThinkingFormat::Zai,
                MaxTokensField::MaxTokens,
            ),
            (
                "https://open.bigmodel.cn/api/paas/v4",
                ThinkingFormat::Zai,
                MaxTokensField::MaxTokens,
            ),
            (
                "https://api.together.xyz/v1",
                ThinkingFormat::Together,
                MaxTokensField::MaxTokens,
            ),
            (
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
                ThinkingFormat::Qwen,
                MaxTokensField::MaxTokens,
            ),
            (
                "https://api.openai.com/v1",
                ThinkingFormat::Openai,
                MaxTokensField::MaxCompletionTokens,
            ),
        ];
        for (base_url, format, max_tokens_field) in cases {
            let model = test_model(base_url);
            let compat = get_compat(&model);
            assert_eq!(compat.thinking_format, *format, "format for {base_url}");
            assert_eq!(
                compat.max_tokens_field, *max_tokens_field,
                "max tokens for {base_url}"
            );
        }
        // 探测细节
        let compat = get_compat(&test_model("https://api.moonshot.cn/v1"));
        assert!(!compat.supports_strict_mode);
        let compat = get_compat(&test_model("https://api.deepseek.com/v1"));
        assert!(compat.requires_reasoning_content_on_assistant_messages);
        let compat = get_compat(&test_model("https://api.x.ai/v1"));
        assert!(!compat.supports_reasoning_effort);
        // 显式 compat 覆盖探测
        let mut model = test_model("https://api.deepseek.com/v1");
        model.compat = Some(OpenAICompletionsCompat {
            thinking_format: Some(ThinkingFormat::Openai),
            ..Default::default()
        });
        assert_eq!(get_compat(&model).thinking_format, ThinkingFormat::Openai);
    }

    // ── 流聚合器 ─────────────────────────────────────────────────────

    fn aggregator_for(base_url: &str) -> StreamAggregator {
        let model = test_model(base_url);
        StreamAggregator::new(&model, get_compat(&model))
    }

    fn chunk(delta: Value, finish_reason: Value) -> Value {
        json!({
            "id": "chatcmpl-1",
            "model": "gpt-test",
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }]
        })
    }

    #[test]
    fn text_stream_produces_ordered_events() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let mut all = aggregator.apply_chunk(&chunk(json!({"content": "Hel"}), json!(null)));
        all.extend(aggregator.apply_chunk(&chunk(json!({"content": "lo"}), json!(null))));
        all.extend(aggregator.apply_chunk(&chunk(json!({}), json!("stop"))));
        let (end_events, message, terminal) = aggregator.finalize(false, None);

        let mut events = all;
        events.extend(end_events);
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AssistantMessageEvent::TextStart { .. } => "text_start",
                AssistantMessageEvent::TextDelta { delta, .. } => {
                    assert!(!delta.is_empty());
                    "text_delta"
                }
                AssistantMessageEvent::TextEnd { .. } => "text_end",
                other => unreachable!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["text_start", "text_delta", "text_delta", "text_end"]
        );
        assert_eq!(message.content[0], AssistantContent::text("Hello"));
        match terminal {
            AssistantMessageEvent::Done { reason, message } => {
                assert_eq!(reason, StopReason::Stop);
                assert_eq!(message.response_id.as_deref(), Some("chatcmpl-1"));
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn tool_calls_aggregate_by_index_with_partial_json() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let mut events = aggregator.apply_chunk(&chunk(
            json!({"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "get_weather", "arguments": ""}}]}),
            json!(null),
        ));
        events.extend(aggregator.apply_chunk(&chunk(
            json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"city\":"}}]}),
            json!(null),
        )));
        events.extend(aggregator.apply_chunk(&chunk(
            json!({"tool_calls": [{"index": 0, "function": {"arguments": " \"Oslo\"}"}}]}),
            json!(null),
        )));
        events.extend(aggregator.apply_chunk(&chunk(json!({}), json!("tool_calls"))));
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolcallStart { .. }
        ));
        assert!(matches!(
            events[1],
            AssistantMessageEvent::ToolcallDelta { .. }
        ));

        let (end_events, _message, terminal) = aggregator.finalize(false, None);
        let Some(AssistantMessageEvent::ToolcallEnd { tool_call, .. }) = end_events
            .iter()
            .find(|e| matches!(e, AssistantMessageEvent::ToolcallEnd { .. }))
        else {
            panic!("expected toolcall_end");
        };
        assert_eq!(tool_call.id, "call_1");
        assert_eq!(tool_call.name, "get_weather");
        assert_eq!(
            tool_call.arguments.get("city").and_then(Value::as_str),
            Some("Oslo")
        );
        match terminal {
            AssistantMessageEvent::Done { reason, .. } => assert_eq!(reason, StopReason::ToolUse),
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn multiple_tool_calls_split_by_index() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(
            json!({"tool_calls": [
                {"index": 0, "id": "call_a", "function": {"name": "a", "arguments": "{}"}},
                {"index": 1, "id": "call_b", "function": {"name": "b", "arguments": "{\"x\":1}"}}
            ]}),
            json!(null),
        ));
        aggregator.apply_chunk(&chunk(json!({}), json!("tool_calls")));
        let (end_events, message, terminal) = aggregator.finalize(false, None);
        let ends: Vec<&AssistantMessageEvent> = end_events
            .iter()
            .filter(|e| matches!(e, AssistantMessageEvent::ToolcallEnd { .. }))
            .collect();
        assert_eq!(ends.len(), 2);
        assert_eq!(message.content.len(), 2);
        let AssistantContent::ToolCall(second) = &message.content[1] else {
            panic!()
        };
        assert_eq!(second.id, "call_b");
        assert_eq!(second.arguments.get("x").and_then(Value::as_i64), Some(1));
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::ToolUse,
                ..
            }
        ));
    }

    #[test]
    fn reasoning_delta_creates_thinking_block() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        let mut events =
            aggregator.apply_chunk(&chunk(json!({"reasoning_content": "hmm"}), json!(null)));
        events.extend(aggregator.apply_chunk(&chunk(json!({"content": "answer"}), json!(null))));
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ThinkingStart { .. }
        ));
        assert!(matches!(
            events[1],
            AssistantMessageEvent::ThinkingDelta { .. }
        ));
        assert!(matches!(events[2], AssistantMessageEvent::TextStart { .. }));

        let (end_events, message, _) = aggregator.finalize(false, None);
        let Some(AssistantMessageEvent::ThinkingEnd { content, .. }) = end_events
            .iter()
            .find(|e| matches!(e, AssistantMessageEvent::ThinkingEnd { .. }))
        else {
            panic!("expected thinking_end");
        };
        assert_eq!(content, "hmm");
        assert_eq!(
            message.content[0],
            AssistantContent::Thinking {
                thinking: "hmm".to_string(),
                thinking_signature: Some("reasoning_content".to_string()),
                redacted: false,
            }
        );
        assert_eq!(message.content[1], AssistantContent::text("answer"));
    }

    #[test]
    fn reasoning_field_fallback_avoids_duplicates() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        // 同时带 reasoning_content 与 reasoning(同文):只取第一个非空
        let events = aggregator.apply_chunk(&chunk(
            json!({"reasoning_content": "a", "reasoning": "a"}),
            json!(null),
        ));
        assert_eq!(events.len(), 2); // thinking_start + thinking_delta
        let (_, message, _) = aggregator.finalize(false, None);
        let AssistantContent::Thinking { thinking, .. } = &message.content[0] else {
            panic!()
        };
        assert_eq!(thinking, "a");
    }

    #[test]
    fn usage_parsed_with_cache_details() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(json!({"content": "hi"}), json!(null)));
        aggregator.apply_chunk(&json!({
            "id": "chatcmpl-1",
            "model": "gpt-test",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 40,
                "prompt_tokens_details": { "cached_tokens": 60 },
                "completion_tokens_details": { "reasoning_tokens": 10 }
            }
        }));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.usage.input, 40); // 100 - 60 缓存命中
        assert_eq!(message.usage.cache_read, 60);
        assert_eq!(message.usage.output, 40);
        assert_eq!(message.usage.total_tokens, 140);
        assert_eq!(message.usage.reasoning, Some(10));
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                ..
            }
        ));
    }

    #[test]
    fn deepseek_style_usage_falls_back_to_cache_hit_tokens() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "prompt_cache_hit_tokens": 40,
                "completion_tokens": 10
            }
        }));
        assert_eq!(aggregator.output().usage.input, 60);
        assert_eq!(aggregator.output().usage.cache_read, 40);
    }

    #[test]
    fn finish_reason_length_maps_and_raw_is_kept() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(json!({"content": "x"}), json!("length")));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::Length);
        assert_eq!(message.raw_stop_reason.as_deref(), Some("length"));
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::Length,
                ..
            }
        ));
    }

    #[test]
    fn content_filter_finish_reason_becomes_error() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(json!({}), json!("content_filter")));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Provider finish_reason: content_filter")
        );
        match terminal {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Error);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("Provider finish_reason: content_filter")
                );
            }
            other => unreachable!("{other:?}"),
        }
    }

    #[test]
    fn aborted_encoding() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(json!({"content": "partial"}), json!(null)));
        let (end_events, message, terminal) = aggregator.finalize(true, None);
        // 中止发生在循环后:先补块 end 事件再编码 error(aborted)
        assert!(end_events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. })));
        match terminal {
            AssistantMessageEvent::Error { reason, error } => {
                assert_eq!(reason, StopReason::Aborted);
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => unreachable!("{other:?}"),
        }
        assert_eq!(message.content[0], AssistantContent::text("partial"));
    }

    #[test]
    fn stream_error_encoding_skips_block_ends() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(json!({"content": "partial"}), json!(null)));
        let (end_events, message, terminal) =
            aggregator.finalize(false, Some("500: boom".to_string()));
        assert!(end_events.is_empty());
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.error_message.as_deref(), Some("500: boom"));
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Error {
                reason: StopReason::Error,
                ..
            }
        ));
    }

    #[test]
    fn missing_finish_reason_errors_by_default() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(json!({"content": "x"}), json!(null)));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Stream ended without finish_reason")
        );
        assert!(matches!(terminal, AssistantMessageEvent::Error { .. }));
    }

    #[test]
    fn missing_finish_reason_falls_back_when_unsupported() {
        let mut model = test_model("https://api.openai.com/v1");
        model.compat = Some(OpenAICompletionsCompat {
            supports_finish_reason: Some(false),
            ..Default::default()
        });
        let mut aggregator = StreamAggregator::new(&model, get_compat(&model));
        aggregator.apply_chunk(&chunk(json!({"content": "x"}), json!(null)));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert!(matches!(terminal, AssistantMessageEvent::Done { .. }));

        // 有 tool call → 兜底 toolUse
        let mut aggregator = StreamAggregator::new(&model, get_compat(&model));
        aggregator.apply_chunk(&chunk(
            json!({"tool_calls": [{"index": 0, "id": "c", "function": {"name": "f", "arguments": "{}"}}]}),
            json!(null),
        ));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert!(matches!(terminal, AssistantMessageEvent::Done { .. }));
    }

    #[test]
    fn response_model_captured_when_different() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&json!({
            "id": "chatcmpl-1",
            "model": "gpt-test-real",
            "choices": [{ "index": 0, "delta": {"content": "hi"}, "finish_reason": "stop" }]
        }));
        assert_eq!(
            aggregator.output().response_model.as_deref(),
            Some("gpt-test-real")
        );
        // 同名模型不记录
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&json!({
            "id": "chatcmpl-1",
            "model": "gpt-test",
            "choices": [{ "index": 0, "delta": {"content": "hi"}, "finish_reason": "stop" }]
        }));
        assert!(aggregator.output().response_model.is_none());
    }

    #[test]
    fn reasoning_details_are_serialized_into_signature() {
        let mut aggregator = aggregator_for("https://api.openai.com/v1");
        aggregator.apply_chunk(&chunk(
            json!({
                "reasoning_details": [
                    {"type": "reasoning.text", "text": "alpha", "id": "r1", "index": 0},
                    {"type": "reasoning.text", "text": "beta", "id": "r1", "index": 0}
                ]
            }),
            json!(null),
        ));
        aggregator.apply_chunk(&chunk(json!({"content": "done"}), json!("stop")));
        let (_, message, _) = aggregator.finalize(false, None);
        let AssistantContent::Thinking {
            thinking,
            thinking_signature,
            ..
        } = &message.content[0]
        else {
            panic!()
        };
        assert_eq!(thinking, "");
        let details: Value = serde_json::from_str(thinking_signature.as_deref().unwrap()).unwrap();
        assert_eq!(details.as_array().unwrap().len(), 1);
        assert_eq!(details[0]["text"], "alphabeta");
    }

    // ── 部分容错 JSON ────────────────────────────────────────────────

    #[test]
    fn parse_streaming_json_object_handles_partial_inputs() {
        let parse = |text: &str| parse_streaming_json_object(text);
        assert_eq!(
            parse(r#"{"a": 1}"#),
            serde_json::from_value::<Map<String, Value>>(json!({"a": 1})).unwrap()
        );
        assert_eq!(
            parse(r#"{"path": "src/ma"#)
                .get("path")
                .and_then(Value::as_str),
            Some("src/ma")
        );
        assert_eq!(
            parse(r#"{"a": 1, "b"#).get("a").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            parse(r#"{"a": [1, 2"#)
                .get("a")
                .and_then(|v| v.as_array())
                .map(|items| items.len()),
            Some(2)
        );
        assert_eq!(
            parse(r#"{"a": {"b": 1"#)
                .get("a")
                .and_then(|v| v.get("b"))
                .and_then(Value::as_i64),
            Some(1)
        );
        assert!(parse(r#"{"a": tru"#).is_empty());
        assert!(parse(r#"{"na"#).is_empty());
        assert!(parse("").is_empty());
        // 转义残片("b\ 结尾):丢弃尾部反斜杠后闭合,不 panic
        assert_eq!(
            parse(r#"{"a": "b\"#).get("a").and_then(Value::as_str),
            Some("b")
        );
        // 完整但带裸控制字符
        assert_eq!(
            parse("{\"a\": \"li\nne\"}")
                .get("a")
                .and_then(Value::as_str),
            Some("li\nne")
        );
    }

    #[test]
    fn tool_arguments_parse_failure_falls_back_to_empty_map() {
        // 数组/标量根 → 空 Map(契约:回退空 Map 不 panic)
        assert!(parse_streaming_json_object("[1, 2]").is_empty());
        assert!(parse_streaming_json_object("\"just a string\"").is_empty());
    }

    #[test]
    fn short_hash_is_stable_and_matches_ts_shape() {
        // 同输入稳定;两段 base36 拼接(仅小写字母数字)
        let first = short_hash("call|item");
        let second = short_hash("call|item");
        assert_eq!(first, second);
        assert!(first
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(!first.is_empty());
    }

    #[test]
    fn normalize_tool_call_id_handles_plain_and_openai_truncation() {
        assert_eq!(
            normalize_tool_call_id("call_simple", "custom"),
            "call_simple"
        );
        let long = "a".repeat(50);
        assert_eq!(normalize_tool_call_id(&long, "openai"), "a".repeat(40));
        assert_eq!(normalize_tool_call_id(&long, "custom"), long);
    }

    #[test]
    fn format_http_error_shapes() {
        // {"error":{"message"}} JSON → "status: message"(与 async-openai ApiError 同形)
        assert_eq!(
            format_http_error(429, r#"{"error":{"message":"rate limited"}}"#),
            "429: rate limited"
        );
        // 非 JSON body → 原文截断(512 字符)
        assert_eq!(
            format_http_error(502, "<html>bad gateway</html>"),
            "502: <html>bad gateway</html>"
        );
        let long = "x".repeat(600);
        let message = format_http_error(500, &long);
        assert_eq!(message, format!("500: {}", "x".repeat(512)));
        // 空 body → 只有状态码
        assert_eq!(format_http_error(503, "  \n"), "503");
    }

    #[test]
    fn estimate_and_clamp_match_blueprint() {
        let model = test_model("https://api.openai.com/v1");
        let mut context = context_of(vec![user_message("abcdefgh")]); // 8 chars → 2 tokens
        context.system_prompt = Some("12345678".to_string()); // 2 tokens
        assert_eq!(estimate_context_tokens(&context), 4);

        // contextWindow <= 0:原样(不低于 1)
        assert_eq!(clamp_max_tokens_to_context(&model, &context, 0), 1);
        assert_eq!(clamp_max_tokens_to_context(&model, &context, 500), 500);

        // 有窗口:受 available = window - estimate - 4096 约束
        let mut model = model;
        model.context_window = 4102; // 4102 - 4 - 4096 = 2
        assert_eq!(clamp_max_tokens_to_context(&model, &context, 100), 2);
    }

    #[test]
    fn thinking_budget_math() {
        assert_eq!(thinking_budget_for_level(ThinkingLevel::High, None), 16384);
        assert_eq!(thinking_budget_for_level(ThinkingLevel::Max, None), 16384);
        assert_eq!(thinking_budget_for_level(ThinkingLevel::Low, None), 2048);
        assert_eq!(clamp_thinking_budget_to_answer_room(16384, 8192), 7168);
        assert_eq!(clamp_thinking_budget_to_answer_room(16384, 512), 0);
        let custom = ThinkingBudgets {
            low: Some(4096),
            ..Default::default()
        };
        assert_eq!(
            thinking_budget_for_level(ThinkingLevel::Low, Some(&custom)),
            4096
        );
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
        // x-should-retry 优先于状态码判定
        assert!(retryable(Some(400), Some("true")));
        assert!(!retryable(Some(500), Some("false")));
        assert!(retryable(None, Some("true")));
        assert!(!retryable(None, Some("false")));
        // 头取其他值时不表态,回落状态码
        assert!(retryable(Some(429), Some("1")));
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
        // 服务端延迟超上限 → 立即失败,文案对齐 TS(向上取整秒)
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

    // ── SSE 解码 ─────────────────────────────────────────────────────

    fn decode_all(decoder: &mut SseDecoder, chunks: &[&[u8]]) -> Vec<ServerSentEvent> {
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.push_bytes(chunk));
        }
        events
    }

    #[test]
    fn sse_decoder_splits_lines_and_joins_multi_data() {
        let mut decoder = SseDecoder::default();
        // 跨块断行 + 多行 data 以 \n 合并
        let events = decode_all(
            &mut decoder,
            &[b"data: Hel", b"lo,\ndata: wor", b"ld!\n\ndata: next\n\n"],
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "Hello,\nworld!");
        assert_eq!(events[0].event, None);
        assert_eq!(events[1].data, "next");
    }

    #[test]
    fn sse_decoder_tolerates_crlf_cr_and_comments() {
        let mut decoder = SseDecoder::default();
        let events = decode_all(
            &mut decoder,
            &[b": keep-alive\r\n\r: ping\r\rdata: a\r\n\r\n"],
        );
        // 注释行忽略;裸 \r 也是行结束;最后一个事件由空行分发
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "a");
        // 无 data 行的空行不分发,event 暂存也被丢弃
        let events = decode_all(&mut decoder, &[b"event: x\n\n"]);
        assert!(events.is_empty());
        let events = decode_all(&mut decoder, &[b"data: b\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "b");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn sse_decoder_keeps_event_type_and_strips_bom() {
        let mut decoder = SseDecoder::default();
        let events = decode_all(
            &mut decoder,
            &[
                "\u{feff}".as_bytes(),
                b"event: keepalive\ndata: ping\n\n",
                b"event: message\ndata: {\"a\":1}\n\n",
            ],
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("keepalive"));
        assert_eq!(events[0].data, "ping");
        assert_eq!(events[1].event.as_deref(), Some("message"));
    }

    #[test]
    fn decode_chunk_payload_semantics() {
        let payload = |event: Option<&str>, data: &str| {
            decode_chunk_payload(&ServerSentEvent {
                event: event.map(str::to_string),
                data: data.to_string(),
            })
        };
        // [DONE] 终止流
        assert!(matches!(payload(None, "[DONE]"), SsePayload::Done));
        // keepalive 事件跳过(即便 data 不是 JSON)
        assert!(matches!(
            payload(Some("keepalive"), "ping"),
            SsePayload::Skip
        ));
        // 普通 JSON chunk
        match payload(None, "{\"choices\":[]}") {
            SsePayload::Chunk(value) => assert!(value.get("choices").is_some()),
            other => panic!("expected chunk, got {other:?}"),
        }
        // 非 JSON data → 流错误(对齐既有 Some(Err) 分支)
        match payload(None, "not json") {
            SsePayload::Fatal(message) => {
                assert!(message.starts_with("failed to deserialize api response:"));
            }
            other => panic!("expected fatal, got {other:?}"),
        }
    }

    // ── 集成:重试管线(本地 TCP mock) ────────────────────────────────

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

    /// 完整的 chat.completions SSE 流:文本增量 → finish_reason stop → [DONE]。
    fn chat_sse_stream() -> String {
        let first = json!({
            "id": "chatcmpl-1",
            "model": "gpt-test",
            "choices": [{"index": 0, "delta": {"content": "Hi"}, "finish_reason": null}]
        });
        let last = json!({
            "id": "chatcmpl-1",
            "model": "gpt-test",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", first, last)
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
        let (addr, handler) = spawn_mock_server(vec![
            http_error_response("500 Internal Server Error", "boom", &[]),
            http_error_response(
                "429 Too Many Requests",
                "slow down",
                &[("retry-after-ms", "50")],
            ),
            http_sse_response(&chat_sse_stream()),
        ]);
        let options = SimpleStreamOptions {
            api_key: Some("k".to_string()),
            max_retries: Some(3),
            ..Default::default()
        };
        let model = test_model(&format!("http://{addr}"));
        let stream = stream_openai_completions(
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
                assert_eq!(message.content[0], AssistantContent::text("Hi"));
                assert_eq!(message.response_id.as_deref(), Some("chatcmpl-1"));
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provider_retry_respects_x_should_retry() {
        let key = || Some("k".to_string());
        // 400 默认不可重试,但 x-should-retry: true 强制重试
        let (addr, handler) = spawn_mock_server(vec![
            http_error_response("400 Bad Request", "maybe", &[("x-should-retry", "true")]),
            http_sse_response(&chat_sse_stream()),
        ]);
        let model = test_model(&format!("http://{addr}"));
        let stream = stream_openai_completions(
            model,
            context_of(vec![user_message("hi")]),
            Some(SimpleStreamOptions {
                api_key: key(),
                ..Default::default() // max_retries 缺省 2
            }),
            None,
        );
        let terminal = collect_terminal(stream).await;
        assert_eq!(handler.join().unwrap(), 2);
        assert!(matches!(
            terminal,
            Some(AssistantMessageEvent::Done {
                reason: StopReason::Stop,
                ..
            })
        ));

        // 5xx 默认可重试,但 x-should-retry: false 立即失败(不建立第二个连接)
        let (addr, handler) = spawn_mock_server(vec![http_error_response(
            "503 Service Unavailable",
            "nope",
            &[("x-should-retry", "false")],
        )]);
        let model = test_model(&format!("http://{addr}"));
        let stream = stream_openai_completions(
            model,
            context_of(vec![user_message("hi")]),
            Some(SimpleStreamOptions {
                api_key: key(),
                ..Default::default()
            }),
            None,
        );
        let terminal = collect_terminal(stream).await;
        assert_eq!(handler.join().unwrap(), 1);
        match terminal {
            Some(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error,
            }) => assert_eq!(error.error_message.as_deref(), Some("503: nope")),
            other => panic!("expected error, got {other:?}"),
        }
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
        let stream = stream_openai_completions(
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
                assert_eq!(error.error_message.as_deref(), Some("408: timeout two"));
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
        let stream = stream_openai_completions(
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
