//! Google Generative AI(Gemini)provider:pi-ai `api/google-generative-ai.ts` +
//! `api/google-shared.ts`(0.84.4)的 Rust 复刻。
//!
//! 组成:
//! - [`build_request_body`]:纯函数,把 [`Model`] + [`Context`] + [`SimpleStreamOptions`]
//!   序列化为 Gemini `generateContent` REST 请求体(contents / systemInstruction /
//!   tools / toolConfig / generationConfig.thinkingConfig);构造失败(非法
//!   thinkingLevelMap 映射)以 `Err` 返回,由 [`stream_google_generative_ai`] 编码进流。
//! - [`stream_google_generative_ai`]:流式入口,reqwest 直连
//!   `models/{model}:streamGenerateContent?alt=sse`,把 Google SSE(data JSON)聚合为
//!   [`AssistantMessageEvent`] 推入 [`EventStreamWriter`];失败/中止编码进流
//!   (stopReason error/aborted + errorMessage),不 panic、不抛出。
//! - thinking:Gemini 2.x 走 thinkingBudget(按模型档位表,支持自定义预算),
//!   Gemini 3 / Gemma 4 走 thinkingLevel;关闭思考按模型降级(2.x budget=0,
//!   3.x 最低档位)。多轮 thoughtSignature 回放:同 provider/model 且 base64
//!   合法才保留;Gemini 3+ 工具调用/结果带显式 id。
//! - 初始连接带 provider 重试策略(408/409/429/5xx + retry-after,对齐蓝本
//!   `retryProviderRequest`);流建立后错误不重试。
//!
//! 与蓝本的已知偏差(语义不变):
//! - `sanitizeSurrogates` 为恒等(Rust String 不可能携带未配对代理对);
//! - `Tool` 类型无 constrainedSampling 元数据 → 严格采样(strict/VALIDATED)恒不可达;
//! - 中立 `ToolChoice` 只有 auto/none(蓝本 GoogleOptions 的 "any" 不经
//!   SimpleStreamOptions 暴露,`map_tool_choice` 保留完整映射);
//! - 抖动随机数用时间熵 xorshift(rand 不在本 crate 直接依赖内);
//! - thinking 参数入口统一走 streamSimple 语义(SimpleStreamOptions.reasoning),
//!   蓝本 GoogleOptions.thinking 显式覆盖不建模。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::DateTime;
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::event_stream::{event_stream, EventStreamWriter};
use super::types::{
    user_agent, AssistantContent, AssistantMessage, AssistantMessageEvent,
    AssistantMessageEventStream, Context, InputKind, Message, Model, ModelThinkingLevel,
    SimpleStreamOptions, StopReason, TextOrImageContent, ThinkingBudgets, ThinkingLevel, Tool,
    ToolCall, ToolChoice, ToolResultMessage, Usage, UsageCost, UserContent,
};
use crate::time_util::now_ts_nanos;

const GOOGLE_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const GOOGLE_DEFAULT_API_VERSION: &str = "v1beta";

const TOOL_RESULT_IMAGE_TEXT: &str = "(see attached image)";
const TOOL_RESULT_IMAGE_TURN_TEXT: &str = "Tool result image:";
const SYNTHETIC_TOOL_RESULT_TEXT: &str = "No result provided";
const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

const MAX_PROVIDER_ERROR_BODY_CHARS: usize = 4000;
const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

const CONTEXT_SAFETY_TOKENS: i64 = 4096;
const MIN_MAX_TOKENS: i64 = 1;
const CHARS_PER_TOKEN: i64 = 4;
const ESTIMATED_IMAGE_CHARS: i64 = 4800;

/// 工具调用合成 id 计数器(TS 模块级 `toolCallCounter`,从 1 起)。
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);
/// 抖动随机数状态(时间熵 xorshift)。
static JITTER_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> i64 {
    now_ts_nanos() / 1_000_000
}

// ── 模型形态判定(google-shared.ts) ───────────────────────────────────

fn gemini_version_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^gemini(?:-live)?-(\d+)").expect("static regex"))
}

fn gemini_major_version(model_id: &str) -> Option<u32> {
    gemini_version_regex()
        .captures(&model_id.to_lowercase())
        .and_then(|captures| captures[1].parse().ok())
}

/// Google API 侧需要显式工具调用 id 的模型(claude-/gpt-oss- 代理与 Gemini 3+)。
fn requires_tool_call_id(model_id: &str) -> bool {
    let gemini_major_version = gemini_major_version(model_id);
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-oss-")
        || gemini_major_version.is_some_and(|version| version >= 3)
}

fn supports_multimodal_function_response(model_id: &str) -> bool {
    match gemini_major_version(model_id) {
        Some(version) => version >= 3,
        None => true,
    }
}

/// Gemini 3+ 在受验证的工具调用模式下强制 required 参数(严格采样)。
fn supports_google_strict_tool_sampling(model_id: &str) -> bool {
    gemini_major_version(model_id).is_some_and(|version| version >= 3)
}

fn is_gemma4_model(model_id: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(r"gemma-?4").expect("static regex"));
    pattern.is_match(&model_id.to_lowercase())
}

fn is_gemini_3_pro_model(model_id: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern =
        PATTERN.get_or_init(|| Regex::new(r"gemini-3(?:\.\d+)?-pro").expect("static regex"));
    pattern.is_match(&model_id.to_lowercase())
}

fn is_gemini_3_flash_model(model_id: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern =
        PATTERN.get_or_init(|| Regex::new(r"gemini-3(?:\.\d+)?-flash").expect("static regex"));
    let id = model_id.to_lowercase();
    pattern.is_match(&id) || id == "gemini-flash-latest" || id == "gemini-flash-lite-latest"
}

// ── thought 签名(google-shared.ts) ───────────────────────────────────

/// 流式增量去重:后端只在块的首个 delta 携带签名,后续 delta 可能省略;
/// 保留同块内最近一个非空签名,不跨块合并/搬移。
fn retain_thought_signature<'a>(
    existing: Option<&'a str>,
    incoming: Option<&'a str>,
) -> Option<&'a str> {
    match incoming {
        Some(value) if !value.is_empty() => Some(value),
        _ => existing,
    }
}

/// 签名必须是 base64(Google API 的 TYPE_BYTES)。
fn is_valid_thought_signature(signature: &str) -> bool {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern =
        PATTERN.get_or_init(|| Regex::new(r"^[A-Za-z0-9+/]+={0,2}$").expect("static regex"));
    !signature.is_empty() && signature.len() % 4 == 0 && pattern.is_match(signature)
}

/// 只保留同 provider/model 且 base64 合法的签名(跨模型签名不可用)。
fn resolve_thought_signature(
    is_same_provider_and_model: bool,
    signature: Option<&str>,
) -> Option<String> {
    let signature = signature.filter(|value| !value.is_empty())?;
    if is_same_provider_and_model && is_valid_thought_signature(signature) {
        Some(signature.to_string())
    } else {
        None
    }
}

// ── thinking 级别(google-shared.ts + google-generative-ai.ts) ─────────

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

fn thinking_level_from(level: ThinkingLevel) -> ModelThinkingLevel {
    match level {
        ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
        ThinkingLevel::Low => ModelThinkingLevel::Low,
        ThinkingLevel::Medium => ModelThinkingLevel::Medium,
        ThinkingLevel::High => ModelThinkingLevel::High,
        ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
        ThinkingLevel::Max => ModelThinkingLevel::Max,
    }
}

/// TS models.ts getSupportedThinkingLevels:map 值 null = 显式禁用,
/// xhigh/max 需显式映射,其余缺省可用。
fn get_supported_thinking_levels(model: &Model) -> Vec<ModelThinkingLevel> {
    const EXTENDED: [ModelThinkingLevel; 7] = [
        ModelThinkingLevel::Off,
        ModelThinkingLevel::Minimal,
        ModelThinkingLevel::Low,
        ModelThinkingLevel::Medium,
        ModelThinkingLevel::High,
        ModelThinkingLevel::Xhigh,
        ModelThinkingLevel::Max,
    ];
    if !model.reasoning {
        return vec![ModelThinkingLevel::Off];
    }
    EXTENDED
        .into_iter()
        .filter(|level| {
            match model
                .thinking_level_map
                .as_ref()
                .and_then(|map| map.get(level_key(*level)))
            {
                Some(None) => false,
                Some(Some(_)) => true,
                None => !matches!(level, ModelThinkingLevel::Xhigh | ModelThinkingLevel::Max),
            }
        })
        .collect()
}

/// TS models.ts clampThinkingLevel:就近回落,先向上再向下,兜底可用首项。
fn clamp_thinking_level(model: &Model, level: ThinkingLevel) -> ModelThinkingLevel {
    const EXTENDED: [ModelThinkingLevel; 7] = [
        ModelThinkingLevel::Off,
        ModelThinkingLevel::Minimal,
        ModelThinkingLevel::Low,
        ModelThinkingLevel::Medium,
        ModelThinkingLevel::High,
        ModelThinkingLevel::Xhigh,
        ModelThinkingLevel::Max,
    ];
    let available = get_supported_thinking_levels(model);
    let requested = thinking_level_from(level);
    if available.contains(&requested) {
        return requested;
    }
    let requested_index = EXTENDED
        .iter()
        .position(|candidate| *candidate == requested)
        .unwrap_or(0);
    for candidate in EXTENDED.into_iter().skip(requested_index) {
        if available.contains(&candidate) {
            return candidate;
        }
    }
    for candidate in EXTENDED.into_iter().take(requested_index).rev() {
        if available.contains(&candidate) {
            return candidate;
        }
    }
    available
        .first()
        .copied()
        .unwrap_or(ModelThinkingLevel::Off)
}

/// TS google-shared.ts resolveGoogleThinkingLevel:off → high;否则按
/// thinkingLevelMap(字符串小写)解析,非法映射报错(编码进错误流)。
fn resolve_google_thinking_level(
    model: &Model,
    level: ModelThinkingLevel,
) -> Result<String, String> {
    if level == ModelThinkingLevel::Off {
        return Ok("high".to_string());
    }
    let key = level_key(level);
    let mapped = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get(key))
        .cloned()
        .flatten();
    let resolved = mapped
        .as_ref()
        .map(|value| value.to_lowercase())
        .unwrap_or_else(|| key.to_string());
    match resolved.as_str() {
        "minimal" | "low" | "medium" | "high" => Ok(resolved),
        _ => Err(format!(
            "Unsupported Google thinking level mapping for {}/{}: {} -> {}",
            model.provider,
            model.id,
            key,
            mapped.unwrap_or_else(|| "undefined".to_string())
        )),
    }
}

/// Gemini 3 / Gemma 4 的档位映射(蓝本 getThinkingLevel)。
fn get_thinking_level(effort: &str, model: &Model) -> &'static str {
    if is_gemini_3_pro_model(&model.id) {
        return match effort {
            "minimal" | "low" => "LOW",
            _ => "HIGH",
        };
    }
    if is_gemma4_model(&model.id) {
        return match effort {
            "minimal" | "low" => "MINIMAL",
            _ => "HIGH",
        };
    }
    match effort {
        "minimal" => "MINIMAL",
        "low" => "LOW",
        "medium" => "MEDIUM",
        _ => "HIGH",
    }
}

fn budget_for_level(level: &str, budgets: [i64; 4]) -> i64 {
    match level {
        "minimal" => budgets[0],
        "low" => budgets[1],
        "medium" => budgets[2],
        _ => budgets[3],
    }
}

/// Gemini 2.x thinkingBudget 档位表(蓝本 getGoogleBudget);-1 = 动态预算。
fn get_google_budget(model: &Model, level: &str, custom: Option<&ThinkingBudgets>) -> i64 {
    let custom_value = custom.and_then(|budgets| match level {
        "minimal" => budgets.minimal,
        "low" => budgets.low,
        "medium" => budgets.medium,
        "high" => budgets.high,
        _ => None,
    });
    if let Some(value) = custom_value {
        return i64::from(value);
    }
    let id = &model.id;
    if id.contains("2.5-pro") {
        return budget_for_level(level, [128, 2048, 8192, 32768]);
    }
    if id.contains("2.5-flash-lite") {
        return budget_for_level(level, [512, 2048, 8192, 24576]);
    }
    if id.contains("2.5-flash") {
        return budget_for_level(level, [128, 2048, 8192, 24576]);
    }
    -1
}

/// 关闭思考的降级配置:Gemini 3.1 Pro / 3 Flash / Gemma 4 不支持完全关闭,
/// 用最低档位(不带 includeThoughts,思考对上层不可见);Gemini 2.x budget=0。
fn get_disabled_thinking_config(model: &Model) -> Value {
    if is_gemini_3_pro_model(&model.id) {
        return json!({ "thinkingLevel": "LOW" });
    }
    if is_gemini_3_flash_model(&model.id) || is_gemma4_model(&model.id) {
        return json!({ "thinkingLevel": "MINIMAL" });
    }
    json!({ "thinkingBudget": 0 })
}

/// generationConfig.thinkingConfig(streamSimple 语义合并进 build_request_body):
/// reasoning=Some 且模型支持推理 → includeThoughts + 档位/预算;
/// reasoning=None 且模型支持推理 → 关闭思考降级;否则不下发。
fn thinking_config_for(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
) -> Result<Option<Value>, String> {
    if !model.reasoning {
        return Ok(None);
    }
    let Some(level) = options.and_then(|options| options.reasoning) else {
        return Ok(Some(get_disabled_thinking_config(model)));
    };
    let clamped = clamp_thinking_level(model, level);
    let resolved = resolve_google_thinking_level(model, clamped)?;
    if is_gemini_3_pro_model(&model.id)
        || is_gemini_3_flash_model(&model.id)
        || is_gemma4_model(&model.id)
    {
        Ok(Some(json!({
            "includeThoughts": true,
            "thinkingLevel": get_thinking_level(&resolved, model),
        })))
    } else {
        Ok(Some(json!({
            "includeThoughts": true,
            "thinkingBudget": get_google_budget(
                model,
                &resolved,
                options.and_then(|options| options.thinking_budgets.as_ref()),
            ),
        })))
    }
}

// ── 消息预处理(TS transform-messages.ts) ─────────────────────────────

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

/// TS transformMessages:非图片模型降级图片;跨模型清理 thinking/签名并归一
/// tool call id(normalize 回调由各 provider 注入);error/aborted 回合不回放;
/// 孤儿 tool call 补合成错误结果。
fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_tool_call_id: &dyn Fn(&str) -> String,
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
                                // 空思考块丢弃
                            } else if is_same_model {
                                content.push(AssistantContent::Thinking {
                                    thinking,
                                    thinking_signature,
                                    redacted,
                                });
                            } else {
                                // 跨模型:thinking 降级为纯文本(不带标签,避免模仿)
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
                                let normalized = normalize_tool_call_id(&tool_call.id);
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
                        timestamp: now_ms(),
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

// ── contents 构造(google-shared.ts convertMessages) ──────────────────

/// 需要显式 id 的模型把工具调用 id 消毒为 `^[a-zA-Z0-9_-]+$` 且 ≤64 字符。
fn normalize_google_tool_call_id(id: &str, model_id: &str) -> String {
    if !requires_tool_call_id(model_id) {
        return id.to_string();
    }
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

fn inline_data_part(data: &str, mime_type: &str) -> Value {
    json!({ "inlineData": { "mimeType": mime_type, "data": data } })
}

/// TS convertMessages:user → "user",assistant → "model",toolResult → 带
/// functionResponse 的 "user" turn(连续结果合并进同一 turn)。
/// sanitizeSurrogates 在 Rust 为恒等(String 不可能携带未配对代理对,省略)。
fn convert_messages(model: &Model, context: &Context) -> Vec<Value> {
    let normalize = |id: &str| normalize_google_tool_call_id(id, &model.id);
    let transformed = transform_messages(&context.messages, model, &normalize);
    let mut contents: Vec<Value> = Vec::new();

    for message in &transformed {
        match message {
            Message::User(user) => {
                let parts: Vec<Value> = match &user.content {
                    UserContent::Text(text) => vec![json!({ "text": text })],
                    UserContent::Blocks(blocks) => blocks
                        .iter()
                        .map(|block| match block {
                            TextOrImageContent::Text { text, .. } => json!({ "text": text }),
                            TextOrImageContent::Image { data, mime_type } => {
                                inline_data_part(data, mime_type)
                            }
                        })
                        .collect(),
                };
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "user", "parts": parts }));
            }
            Message::Assistant(assistant) => {
                // 签名回放判定比 transformMessages 宽松:只比 provider + model(蓝本一致)
                let is_same_provider_and_model =
                    assistant.provider == model.provider && assistant.model == model.id;
                let mut parts: Vec<Value> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantContent::Text {
                            text,
                            text_signature,
                        } => {
                            let signature = resolve_thought_signature(
                                is_same_provider_and_model,
                                text_signature.as_deref(),
                            );
                            // 空文本块仅在有签名时保留(Gemini 可把签名挂在空 part 上)
                            if text.trim().is_empty() && signature.is_none() {
                                continue;
                            }
                            let mut part = Map::new();
                            part.insert("text".to_string(), json!(text));
                            if let Some(signature) = &signature {
                                part.insert("thoughtSignature".to_string(), json!(signature));
                            }
                            parts.push(Value::Object(part));
                        }
                        AssistantContent::Thinking {
                            thinking,
                            thinking_signature,
                            ..
                        } => {
                            if is_same_provider_and_model {
                                let signature = resolve_thought_signature(
                                    is_same_provider_and_model,
                                    thinking_signature.as_deref(),
                                );
                                if thinking.trim().is_empty() && signature.is_none() {
                                    continue;
                                }
                                let mut part = Map::new();
                                part.insert("thought".to_string(), json!(true));
                                part.insert("text".to_string(), json!(thinking));
                                if let Some(signature) = &signature {
                                    part.insert("thoughtSignature".to_string(), json!(signature));
                                }
                                parts.push(Value::Object(part));
                            } else {
                                // 跨 provider/model:降级纯文本,签名不可用,空块丢弃
                                if thinking.trim().is_empty() {
                                    continue;
                                }
                                parts.push(json!({ "text": thinking }));
                            }
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            let signature = resolve_thought_signature(
                                is_same_provider_and_model,
                                tool_call.thought_signature.as_deref(),
                            );
                            let mut call = Map::new();
                            call.insert("name".to_string(), json!(tool_call.name));
                            call.insert(
                                "args".to_string(),
                                Value::Object(tool_call.arguments.clone()),
                            );
                            if requires_tool_call_id(&model.id) {
                                call.insert("id".to_string(), json!(tool_call.id));
                            }
                            let mut part = Map::new();
                            part.insert("functionCall".to_string(), Value::Object(call));
                            if let Some(signature) = &signature {
                                part.insert("thoughtSignature".to_string(), json!(signature));
                            }
                            parts.push(Value::Object(part));
                        }
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult(result) => {
                let text_result = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        TextOrImageContent::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let image_content: Vec<(&str, &str)> = if model.input.contains(&InputKind::Image) {
                    result
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            TextOrImageContent::Image { data, mime_type } => {
                                Some((data.as_str(), mime_type.as_str()))
                            }
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let has_text = !text_result.is_empty();
                let has_images = !image_content.is_empty();
                let multimodal = supports_multimodal_function_response(&model.id);

                // 成功用 "output" 键,错误用 "error" 键(SDK 文档约定)
                let response_value = if has_text {
                    json!(text_result)
                } else if has_images {
                    json!(TOOL_RESULT_IMAGE_TEXT)
                } else {
                    json!("")
                };
                let image_parts: Vec<Value> = image_content
                    .iter()
                    .map(|(data, mime_type)| inline_data_part(data, mime_type))
                    .collect();

                let mut function_response = Map::new();
                function_response.insert("name".to_string(), json!(result.tool_name));
                function_response.insert(
                    "response".to_string(),
                    if result.is_error {
                        json!({ "error": response_value })
                    } else {
                        json!({ "output": response_value })
                    },
                );
                if has_images && multimodal {
                    function_response
                        .insert("parts".to_string(), Value::Array(image_parts.clone()));
                }
                if requires_tool_call_id(&model.id) {
                    function_response.insert("id".to_string(), json!(result.tool_call_id));
                }
                let function_response_part =
                    json!({ "functionResponse": Value::Object(function_response) });

                // 所有 functionResponse 合并进单个 user turn(Cloud Code Assist 要求)
                let mut merged = false;
                if let Some(last) = contents.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("user") {
                        if let Some(parts) = last.get_mut("parts").and_then(Value::as_array_mut) {
                            if parts
                                .iter()
                                .any(|part| part.get("functionResponse").is_some())
                            {
                                parts.push(function_response_part.clone());
                                merged = true;
                            }
                        }
                    }
                }
                if !merged {
                    contents.push(json!({ "role": "user", "parts": [function_response_part] }));
                }

                // Gemini < 3:图片放独立的 user turn
                if has_images && !multimodal {
                    let mut parts = vec![json!({ "text": TOOL_RESULT_IMAGE_TURN_TEXT })];
                    parts.extend(image_parts);
                    contents.push(json!({ "role": "user", "parts": parts }));
                }
            }
        }
    }

    contents
}

// ── 工具声明(google-shared.ts convertTools) ──────────────────────────

const JSON_SCHEMA_META_DECLARATIONS: [&str; 8] = [
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions", // draft-2019-09 之前的 $defs 等价物
];

/// 递归剥离 JSON Schema 元声明(legacy `parameters` OpenAPI 路径用)。
fn sanitize_for_open_api(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let mut result = Map::new();
    for (key, value) in object {
        if JSON_SCHEMA_META_DECLARATIONS.contains(&key.as_str()) {
            continue;
        }
        result.insert(key.clone(), sanitize_for_open_api(value));
    }
    Value::Object(result)
}

/// Rust `Tool` 无 constrainedSampling 元数据 → resolveJsonSchemaStrictSampling
/// 恒 None(严格采样工具集为空;保留蓝本结构以便对齐)。
fn resolve_json_schema_strict_sampling(_tool: &Tool, _supports_strict_mode: bool) -> Option<bool> {
    None
}

/// 工具 → Gemini functionDeclarations;useParameters=true 时走 legacy
/// `parameters`(OpenAPI 3.03 子集,带元声明清洗),否则 `parametersJsonSchema`
/// 透传完整 JSON Schema(蓝本 generative-ai 路径)。
fn convert_tools(
    tools: &[Tool],
    use_parameters: bool,
    supports_strict_mode: bool,
) -> Option<Value> {
    if tools.is_empty() {
        return None;
    }
    let declarations: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let strict = resolve_json_schema_strict_sampling(tool, supports_strict_mode);
            // strict 恒 None → 原始 schema
            let parameters = if strict == Some(true) {
                sanitize_for_open_api(&tool.parameters)
            } else {
                tool.parameters.clone()
            };
            let mut declaration = Map::new();
            declaration.insert("name".to_string(), json!(tool.name));
            declaration.insert("description".to_string(), json!(tool.description));
            if use_parameters {
                declaration.insert("parameters".to_string(), sanitize_for_open_api(&parameters));
            } else {
                declaration.insert("parametersJsonSchema".to_string(), parameters);
            }
            Value::Object(declaration)
        })
        .collect();
    Some(json!([{ "functionDeclarations": declarations }]))
}

/// 工具选择 → Gemini FunctionCallingConfigMode 字符串。
fn map_tool_choice(choice: GoogleToolChoice) -> &'static str {
    match choice {
        GoogleToolChoice::Auto => "AUTO",
        GoogleToolChoice::None => "NONE",
        GoogleToolChoice::Any => "ANY",
    }
}

/// 蓝本 GoogleOptions.toolChoice 的完整档位(中立 ToolChoice 无 any)。
enum GoogleToolChoice {
    Auto,
    None,
    Any,
}

fn resolve_google_function_calling_mode(
    tools: &[Tool],
    tool_choice: Option<ToolChoice>,
    supports_strict_mode: bool,
) -> Option<&'static str> {
    let use_strict_mode = tools
        .iter()
        .any(|tool| resolve_json_schema_strict_sampling(tool, supports_strict_mode) == Some(true));
    match tool_choice {
        Some(ToolChoice::None) => Some(map_tool_choice(GoogleToolChoice::None)),
        _ if use_strict_mode => Some("VALIDATED"),
        Some(choice) => Some(map_tool_choice(match choice {
            ToolChoice::Auto => GoogleToolChoice::Auto,
            ToolChoice::None => GoogleToolChoice::None,
        })),
        None => None,
    }
}

// ── 上下文预算估算(TS utils/estimate.ts)与费用 ───────────────────────

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
    (chars + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

fn estimate_tools_tokens<'a>(tools: impl Iterator<Item = &'a Tool>) -> i64 {
    let list: Vec<&Tool> = tools.collect();
    if list.is_empty() {
        return 0;
    }
    let serialized = serde_json::to_string(&list).unwrap_or_default();
    (serialized.chars().count() as i64 + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

/// TS estimateContextTokens:优先最近可用 assistant usage + 尾部增量。
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
                .map(|prompt| {
                    (prompt.chars().count() as i64 + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
                })
                .unwrap_or(0);
            let tools = estimate_tools_tokens(context.tools.iter());
            messages + system + tools
        }
    }
}

/// TS simple-options.ts clampMaxTokensToContext:maxTokens 恒 ≥ 1。
fn clamp_max_tokens_to_context(model: &Model, context: &Context, max_tokens: i64) -> i64 {
    if model.context_window <= 0 {
        return max_tokens.max(MIN_MAX_TOKENS);
    }
    let available = model.context_window - estimate_context_tokens(context) - CONTEXT_SAFETY_TOKENS;
    max_tokens.min(available.max(MIN_MAX_TOKENS))
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

// ── 请求体构造(google-generative-ai.ts buildParams) ──────────────────

/// streamGenerateContent 端点;model.baseUrl 已含版本路径时不追加 apiVersion。
fn build_stream_url(model: &Model) -> String {
    let path = format!("models/{}:streamGenerateContent?alt=sse", model.id);
    if model.base_url.trim().is_empty() {
        format!("{GOOGLE_DEFAULT_BASE_URL}/{GOOGLE_DEFAULT_API_VERSION}/{path}")
    } else {
        format!("{}/{path}", model.base_url.trim_end_matches('/'))
    }
}

/// 纯函数:构造 Gemini `generateContent` REST 请求体。
/// `config`(GenerateContentConfig)按 SDK REST 序列化拆分:systemInstruction /
/// tools / toolConfig 在顶层,temperature / maxOutputTokens / thinkingConfig 在
/// generationConfig 内;model id 经 URL 承载,不进 body。
/// thinkingLevelMap 非法映射时返回 Err(编码为错误流)。
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: Option<&SimpleStreamOptions>,
) -> Result<Value, String> {
    let contents = convert_messages(model, context);

    let mut generation_config = Map::new();
    if let Some(temperature) = options.and_then(|options| options.temperature) {
        generation_config.insert("temperature".to_string(), json!(temperature));
    }
    // buildBaseOptions:maxTokens 恒为 clamp(options.maxTokens ?? model.maxTokens)
    let max_tokens = clamp_max_tokens_to_context(
        model,
        context,
        options
            .and_then(|options| options.max_tokens)
            .map(i64::from)
            .unwrap_or(model.max_tokens),
    );
    generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));

    let supports_strict_mode = supports_google_strict_tool_sampling(&model.id);
    let function_calling_mode = if context.tools.is_empty() {
        None
    } else {
        resolve_google_function_calling_mode(
            &context.tools,
            options.and_then(|options| options.tool_choice),
            supports_strict_mode,
        )
    };

    let mut body = Map::new();
    body.insert("contents".to_string(), Value::Array(contents));
    if let Some(system_prompt) = &context.system_prompt {
        // sanitizeSurrogates 在 Rust 为恒等(见模块注释)
        body.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": system_prompt }] }),
        );
    }
    if !context.tools.is_empty() {
        if let Some(tools) = convert_tools(&context.tools, false, supports_strict_mode) {
            body.insert("tools".to_string(), tools);
        }
    }
    if let Some(mode) = function_calling_mode {
        body.insert(
            "toolConfig".to_string(),
            json!({ "functionCallingConfig": { "mode": mode } }),
        );
    }
    if let Some(thinking_config) = thinking_config_for(model, options)? {
        generation_config.insert("thinkingConfig".to_string(), thinking_config);
    }
    if !generation_config.is_empty() {
        body.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }

    Ok(Value::Object(body))
}

// ── SSE 解码(Google `data: {json}` 流) ───────────────────────────────

/// 增量 SSE 解码器:按行缓冲(容忍 UTF-8 多字节跨 chunk),空行分发事件;
/// 多个 data 行以 "\n" 连接;event/id/注释行忽略;EOF 处的未完结事件按规范丢弃。
#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseDecoder {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(position) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line_bytes: Vec<u8> = self.buffer.drain(..=position).collect();
            let cow = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]);
            let line: &str = cow.strip_suffix('\r').unwrap_or(&cow);
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    events.push(std::mem::take(&mut self.data_lines).join("\n"));
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                self.data_lines.push(rest.to_string());
            }
        }
        events
    }
}

// ── chunk 解析与停止原因 ──────────────────────────────────────────────

/// TS mapStopReasonString:STOP → stop,MAX_TOKENS → length,其余 → error。
fn map_stop_reason_string(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}

/// TS usageMetadata 投影:input 扣除缓存命中,output 含思考 tokens。
fn parse_usage_metadata(metadata: &Value, model: &Model) -> Usage {
    let number_of = |key: &str| metadata.get(key).and_then(Value::as_i64).unwrap_or(0);
    let prompt_tokens = number_of("promptTokenCount");
    let cached_tokens = number_of("cachedContentTokenCount");
    let candidates_tokens = number_of("candidatesTokenCount");
    let thoughts_tokens = number_of("thoughtsTokenCount");

    let mut usage = Usage {
        input: prompt_tokens - cached_tokens,
        output: candidates_tokens + thoughts_tokens,
        cache_read: cached_tokens,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: Some(thoughts_tokens),
        total_tokens: number_of("totalTokenCount"),
        cost: UsageCost::default(),
    };
    calculate_cost(model, &mut usage);
    usage
}

/// 时间熵 xorshift 伪随机 [0, 1)(rand 不在本 crate 直接依赖内的替代)。
fn next_unit_interval() -> f64 {
    let sequence = JITTER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut state = (now_ts_nanos() as u64) ^ sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    (state >> 11) as f64 / (1u64 << 53) as f64
}

// ── 流聚合器(chunk → 事件,便于单测) ─────────────────────────────────

#[derive(Clone, Copy)]
enum OpenBlock {
    Text(usize),
    Thinking(usize),
}

/// Google SSE chunk → AssistantMessageEvent 的纯聚合逻辑(对齐 TS stream 循环)。
/// 事件序列契约:先 `start`(由 [`stream_google_generative_ai`] 发出),
/// 终止于 `done` 或 `error`;文本/思考块按 part 切换即时收尾,toolCall 三连发。
struct StreamAggregator {
    model: Model,
    output: AssistantMessage,
    open: Option<OpenBlock>,
}

impl StreamAggregator {
    fn new(model: &Model) -> Self {
        Self {
            model: model.clone(),
            output: new_assistant_message(model),
            open: None,
        }
    }

    fn output(&self) -> &AssistantMessage {
        &self.output
    }

    fn final_message(&self) -> AssistantMessage {
        self.output.clone()
    }

    fn has_tool_call(&self) -> bool {
        self.output
            .content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall(_)))
    }

    fn has_tool_call_with_id(&self, id: &str) -> bool {
        self.output.content.iter().any(
            |block| matches!(block, AssistantContent::ToolCall(tool_call) if tool_call.id == id),
        )
    }

    fn close_open_block(&mut self, events: &mut Vec<AssistantMessageEvent>) {
        match self.open.take() {
            None => {}
            Some(OpenBlock::Text(index)) => {
                let content = match self.output.content.get(index) {
                    Some(AssistantContent::Text { text, .. }) => text.clone(),
                    _ => String::new(),
                };
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: index as u32,
                    content,
                    partial: self.output.clone(),
                });
            }
            Some(OpenBlock::Thinking(index)) => {
                let content = match self.output.content.get(index) {
                    Some(AssistantContent::Thinking { thinking, .. }) => thinking.clone(),
                    _ => String::new(),
                };
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: index as u32,
                    content,
                    partial: self.output.clone(),
                });
            }
        }
    }

    /// 消费一个 GenerateContentResponse chunk,返回由此产生的事件(顺序对齐 TS)。
    fn apply_chunk(&mut self, chunk: &Value) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        let Some(object) = chunk.as_object() else {
            return events;
        };

        // responseId 为输出专用标识,保留首个非空值
        if self.output.response_id.as_deref().is_none_or(str::is_empty) {
            if let Some(id) = object.get("responseId").and_then(Value::as_str) {
                if !id.is_empty() {
                    self.output.response_id = Some(id.to_string());
                }
            }
        }

        let candidate = object
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first());

        if let Some(parts) = candidate
            .and_then(|candidate| candidate.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
        {
            for part in parts {
                let thought_signature = part
                    .get("thoughtSignature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());

                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    // thought:true 才是思考内容;thoughtSignature 不代表思考块
                    let is_thinking = part.get("thought").and_then(Value::as_bool) == Some(true);
                    let needs_new_block = match (&self.open, is_thinking) {
                        (None, _) => true,
                        (Some(OpenBlock::Text(_)), true)
                        | (Some(OpenBlock::Thinking(_)), false) => true,
                        (Some(OpenBlock::Text(_)), false)
                        | (Some(OpenBlock::Thinking(_)), true) => false,
                    };
                    if needs_new_block {
                        self.close_open_block(&mut events);
                        if is_thinking {
                            self.output.content.push(AssistantContent::Thinking {
                                thinking: String::new(),
                                thinking_signature: None,
                                redacted: false,
                            });
                            let index = self.output.content.len() - 1;
                            self.open = Some(OpenBlock::Thinking(index));
                            events.push(AssistantMessageEvent::ThinkingStart {
                                content_index: index as u32,
                                partial: self.output.clone(),
                            });
                        } else {
                            self.output.content.push(AssistantContent::Text {
                                text: String::new(),
                                text_signature: None,
                            });
                            let index = self.output.content.len() - 1;
                            self.open = Some(OpenBlock::Text(index));
                            events.push(AssistantMessageEvent::TextStart {
                                content_index: index as u32,
                                partial: self.output.clone(),
                            });
                        }
                    }
                    let Some(index) = (match self.open {
                        Some(OpenBlock::Text(index)) | Some(OpenBlock::Thinking(index)) => {
                            Some(index)
                        }
                        None => None,
                    }) else {
                        continue;
                    };
                    if is_thinking {
                        if let Some(AssistantContent::Thinking {
                            thinking,
                            thinking_signature,
                            ..
                        }) = self.output.content.get_mut(index)
                        {
                            thinking.push_str(text);
                            *thinking_signature = retain_thought_signature(
                                thinking_signature.as_deref(),
                                thought_signature,
                            )
                            .map(str::to_string);
                        }
                        events.push(AssistantMessageEvent::ThinkingDelta {
                            content_index: index as u32,
                            delta: text.to_string(),
                            partial: self.output.clone(),
                        });
                    } else {
                        if let Some(AssistantContent::Text {
                            text: target,
                            text_signature,
                        }) = self.output.content.get_mut(index)
                        {
                            target.push_str(text);
                            *text_signature = retain_thought_signature(
                                text_signature.as_deref(),
                                thought_signature,
                            )
                            .map(str::to_string);
                        }
                        events.push(AssistantMessageEvent::TextDelta {
                            content_index: index as u32,
                            delta: text.to_string(),
                            partial: self.output.clone(),
                        });
                    }
                }

                if let Some(function_call) =
                    part.get("functionCall").filter(|value| value.is_object())
                {
                    self.close_open_block(&mut events);

                    let name = function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let provided_id = function_call
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    // 未提供 id 或与已有 toolCall 重复时生成唯一 id
                    let needs_new_id = provided_id
                        .as_deref()
                        .map(|id| self.has_tool_call_with_id(id))
                        .unwrap_or(true);
                    let tool_call_id = if needs_new_id {
                        let counter = TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
                        format!("{name}_{}_{counter}", now_ms())
                    } else {
                        provided_id.clone().unwrap_or_default()
                    };
                    let arguments = function_call
                        .get("args")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();

                    let tool_call = ToolCall {
                        id: tool_call_id,
                        name,
                        arguments,
                        thought_signature: thought_signature.map(str::to_string),
                        namespace: None,
                    };
                    self.output
                        .content
                        .push(AssistantContent::ToolCall(tool_call.clone()));
                    let index = self.output.content.len() - 1;
                    events.push(AssistantMessageEvent::ToolcallStart {
                        content_index: index as u32,
                        partial: self.output.clone(),
                    });
                    events.push(AssistantMessageEvent::ToolcallDelta {
                        content_index: index as u32,
                        delta: serde_json::to_string(&tool_call.arguments)
                            .unwrap_or_else(|_| "{}".to_string()),
                        partial: self.output.clone(),
                    });
                    events.push(AssistantMessageEvent::ToolcallEnd {
                        content_index: index as u32,
                        tool_call,
                        partial: self.output.clone(),
                    });
                }
            }
        }

        if let Some(finish_reason) = candidate
            .and_then(|candidate| candidate.get("finishReason"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.output.raw_stop_reason = Some(finish_reason.to_string());
            self.output.stop_reason = map_stop_reason_string(finish_reason);
            // 有工具调用却报 STOP:归一为 toolUse
            if self.has_tool_call() && self.output.stop_reason == StopReason::Stop {
                self.output.stop_reason = StopReason::ToolUse;
            }
        }

        if let Some(usage) = object
            .get("usageMetadata")
            .filter(|value| value.is_object())
        {
            self.output.usage = parse_usage_metadata(usage, &self.model);
        }

        events
    }

    fn prepare_error(&mut self, reason: StopReason, message: String) {
        self.output.stop_reason = reason;
        self.output.error_message = Some(message);
    }

    fn error_event(&self, reason: StopReason) -> AssistantMessageEvent {
        AssistantMessageEvent::Error {
            reason,
            error: self.output.clone(),
        }
    }

    /// 终态判定(对齐 TS stream 循环后的检查顺序):
    /// 流错误 → error(不补发块 end);正常收尾 → 关闭打开的块 →
    /// 中止 / 缺 finishReason / provider 错误停止 → error;其余 → done。
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

        let mut events = Vec::new();
        self.close_open_block(&mut events);

        if aborted {
            self.prepare_error(StopReason::Aborted, "Request was aborted".to_string());
            let event = self.error_event(StopReason::Aborted);
            return (events, self.output, event);
        }

        if self.output.stop_reason == StopReason::Pending {
            self.prepare_error(
                StopReason::Error,
                "Google stream ended without a finish reason".to_string(),
            );
            let event = self.error_event(StopReason::Error);
            return (events, self.output, event);
        }

        if matches!(
            self.output.stop_reason,
            StopReason::Aborted | StopReason::Error
        ) {
            let message = self
                .output
                .raw_stop_reason
                .as_ref()
                .map(|raw| format!("Provider stopped with: {raw}"))
                .unwrap_or_else(|| "An unknown error occurred".to_string());
            self.prepare_error(StopReason::Error, message);
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
        timestamp: now_ms(),
    }
}

// ── HTTP 客户端与重试(TS createClient + retryGoogleRequest) ───────────

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
            _ => eprintln!("[agent/llm/google] 忽略非法自定义 header: {key}"),
        }
    }
}

/// 请求头:x-goog-api-key + pi UA 打底,model.headers / options.headers 依次覆盖
/// (对齐 TS providerHeadersToRecord 合并顺序)。
fn build_client(
    model: &Model,
    options: Option<&SimpleStreamOptions>,
    api_key: &str,
) -> Result<reqwest::Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = reqwest::header::HeaderValue::from_str(api_key) {
        headers.insert("x-goog-api-key", value);
    }
    if let Ok(value) = reqwest::header::HeaderValue::from_str(&user_agent()) {
        headers.insert(reqwest::header::USER_AGENT, value);
    }
    push_custom_headers(model.headers.as_ref(), &mut headers);
    push_custom_headers(
        options.and_then(|options| options.headers.as_ref()),
        &mut headers,
    );

    let mut builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(15));
    if !headers.is_empty() {
        builder = builder.default_headers(headers);
    }
    if let Some(timeout_ms) = options.and_then(|options| options.timeout_ms) {
        builder = builder.timeout(Duration::from_millis(timeout_ms));
    }
    builder
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))
}

/// TS truncateErrorText:超长 body 截断并标注省略字符数。
fn truncate_error_text(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let mut result: String = text.chars().take(max_chars).collect();
    result.push_str(&format!("... [truncated {} chars]", count - max_chars));
    result
}

/// 初始连接失败分类:Aborted(取消)/ Fatal(不可重试)/ Retryable(可重试,
/// 携带响应头供 retry-after 计算;reqwest 传输错误无状态码,蓝本视为可重试)。
enum ConnectFailure {
    Aborted,
    Fatal(String),
    Retryable {
        message: String,
        headers: Option<reqwest::header::HeaderMap>,
    },
}

async fn send_attempt(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    signal: Option<&CancellationToken>,
) -> Result<reqwest::Response, ConnectFailure> {
    let send = client.post(url).json(body).send();
    let result = if let Some(token) = signal {
        tokio::select! {
            result = send => result,
            _ = token.cancelled() => return Err(ConnectFailure::Aborted),
        }
    } else {
        send.await
    };
    match result {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            let text = truncate_error_text(text.trim(), MAX_PROVIDER_ERROR_BODY_CHARS);
            let message = if text.is_empty() {
                status.as_u16().to_string()
            } else {
                format!("{}: {}", status.as_u16(), text)
            };
            let should_retry = match headers
                .get("x-should-retry")
                .and_then(|value| value.to_str().ok())
            {
                Some("true") => true,
                Some("false") => false,
                _ => {
                    let code = status.as_u16();
                    code == 408 || code == 409 || code == 429 || code >= 500
                }
            };
            if should_retry {
                Err(ConnectFailure::Retryable {
                    message,
                    headers: Some(headers),
                })
            } else {
                Err(ConnectFailure::Fatal(message))
            }
        }
        Err(error) => Err(ConnectFailure::Retryable {
            message: error.to_string(),
            headers: None,
        }),
    }
}

/// TS validateServerRetryDelayMs:服务端要求的延迟超过上限即失败(0 = 不限)。
fn validate_server_retry_delay(
    delay_ms: f64,
    max_retry_delay_ms: Option<u64>,
    provider_error_message: &str,
) -> Result<u64, String> {
    let max_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS) as f64;
    if max_delay_ms > 0.0 && delay_ms > max_delay_ms {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s). {}",
            (delay_ms / 1000.0).ceil(),
            (max_delay_ms / 1000.0).ceil(),
            provider_error_message
        ));
    }
    Ok(delay_ms.max(0.0) as u64)
}

/// TS getRetryDelayMs:retry-after-ms → retry-after(秒 / HTTP 日期)→
/// 指数退避 min(0.5 * 2^retryIndex, 8)s ×(1 - 随机 0~25%)。
fn compute_retry_delay(
    headers: Option<&reqwest::header::HeaderMap>,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
    provider_error_message: &str,
) -> Result<u64, String> {
    let header_value = |name: &str| {
        headers
            .and_then(|headers| headers.get(name))
            .and_then(|value| value.to_str().ok())
    };
    if let Some(value) = header_value("retry-after-ms").and_then(|value| value.parse::<f64>().ok())
    {
        return validate_server_retry_delay(value, max_retry_delay_ms, provider_error_message);
    }
    if let Some(value) = header_value("retry-after") {
        let delay_ms = match value.parse::<f64>() {
            Ok(seconds) => seconds * 1000.0,
            Err(_) => DateTime::parse_from_rfc2822(value)
                .map(|date| (date.timestamp_millis() - now_ms()).max(0) as f64)
                .unwrap_or(0.0),
        };
        return validate_server_retry_delay(delay_ms, max_retry_delay_ms, provider_error_message);
    }
    let exponential = 0.5f64 * 2f64.powi(retry_index as i32);
    let delay_ms = exponential.min(8.0) * 1000.0 * (1.0 - next_unit_interval() * 0.25);
    validate_server_retry_delay(delay_ms, max_retry_delay_ms, provider_error_message)
}

// ── 流式入口 ──────────────────────────────────────────────────────────

/// Google Generative AI 流式生成:返回事件流(先 `start`,终止于 `done`/`error`)。
/// 失败/中止编码为 stopReason error/aborted 的最终消息,不 panic;
/// `signal` 取消即时生效(连接期、重试退避期与读取期)。
pub fn stream_google_generative_ai(
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
    let is_aborted = || signal.as_ref().is_some_and(|token| token.is_cancelled());
    let mut aggregator = StreamAggregator::new(&model);
    writer.push(AssistantMessageEvent::Start {
        partial: aggregator.output().clone(),
    });

    // 早退错误:reason 取当下取消状态(对齐 TS catch 的 signal.aborted 判定)
    macro_rules! finish_error {
        ($message:expr) => {{
            let reason = if is_aborted() {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            aggregator.prepare_error(reason, $message);
            writer.push(aggregator.error_event(reason));
            writer.end(aggregator.final_message());
            return;
        }};
    }

    if is_aborted() {
        finish_error!("Request was aborted".to_string());
    }

    // TS 顺序:apiKey → client → buildParams → onPayload → 重试连接
    let Some(api_key) = options
        .as_ref()
        .and_then(|options| options.api_key.clone())
        .filter(|key| !key.is_empty())
    else {
        finish_error!(format!("No API key for provider: {}", model.provider));
    };
    let client = match build_client(&model, options.as_ref(), &api_key) {
        Ok(client) => client,
        Err(message) => finish_error!(message),
    };

    let mut body = match build_request_body(&model, &context, options.as_ref()) {
        Ok(body) => body,
        Err(message) => finish_error!(message),
    };
    if let Some(on_payload) = options
        .as_ref()
        .and_then(|options| options.on_payload.as_ref())
    {
        if let Some(next) = on_payload(body.clone()).await {
            body = next;
        }
    }
    let url = build_stream_url(&model);

    // 初始请求重试(蓝本 retryGoogleRequest → retryProviderRequest)
    let max_retries = options
        .as_ref()
        .and_then(|options| options.max_retries)
        .unwrap_or(0);
    let mut retries_remaining = max_retries;
    let response = loop {
        match send_attempt(&client, &url, &body, signal.as_ref()).await {
            Ok(response) => break Ok(response),
            Err(ConnectFailure::Aborted) => {
                break Err(("Request was aborted".to_string(), true));
            }
            Err(ConnectFailure::Fatal(message)) => break Err((message, false)),
            Err(ConnectFailure::Retryable { message, headers }) => {
                if is_aborted() {
                    break Err(("Request was aborted".to_string(), true));
                }
                if retries_remaining == 0 {
                    break Err((message, false));
                }
                let retry_index = max_retries - retries_remaining;
                retries_remaining -= 1;
                let delay = match compute_retry_delay(
                    headers.as_ref(),
                    retry_index,
                    options
                        .as_ref()
                        .and_then(|options| options.max_retry_delay_ms),
                    &message,
                ) {
                    Ok(delay) => delay,
                    Err(validation) => break Err((validation, false)),
                };
                let slept = match &signal {
                    Some(token) => {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(delay)) => true,
                            _ = token.cancelled() => false,
                        }
                    }
                    None => {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        true
                    }
                };
                if !slept {
                    break Err(("Request was aborted".to_string(), true));
                }
            }
        }
    };
    let mut response = match response {
        Ok(response) => response,
        Err((message, aborted)) => {
            let reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            aggregator.prepare_error(reason, message);
            writer.push(aggregator.error_event(reason));
            writer.end(aggregator.final_message());
            return;
        }
    };

    // SSE 读取循环(流建立后错误不重试)
    let mut decoder = SseDecoder::default();
    let mut aborted = false;
    let mut stream_error: Option<String> = None;
    loop {
        let chunk_result = if let Some(token) = &signal {
            tokio::select! {
                chunk = response.chunk() => chunk,
                _ = token.cancelled() => {
                    aborted = true;
                    break;
                }
            }
        } else {
            response.chunk().await
        };
        match chunk_result {
            Ok(Some(bytes)) => {
                for payload in decoder.push(&bytes) {
                    if payload.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(&payload) {
                        Ok(chunk) => {
                            for event in aggregator.apply_chunk(&chunk) {
                                writer.push(event);
                            }
                        }
                        Err(error) => {
                            stream_error =
                                Some(format!("Failed to parse Google stream chunk: {error}"));
                            break;
                        }
                    }
                }
                if stream_error.is_some() {
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                stream_error = Some(error.to_string());
                break;
            }
        }
    }
    // 流正常结束但 signal 已中止(对齐 TS 循环后的 signal.aborted 检查)
    if stream_error.is_none() && !aborted && is_aborted() {
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
    use crate::agent::llm::API_GOOGLE_GENERATIVE_AI;
    use futures::StreamExt;

    fn google_model(id: &str) -> Model {
        let mut model = Model::from_settings(id, GOOGLE_DEFAULT_BASE_URL);
        model.api = API_GOOGLE_GENERATIVE_AI.to_string();
        model.provider = "google".to_string();
        model.reasoning = true;
        model.input = vec![InputKind::Text, InputKind::Image];
        model.max_tokens = 8192;
        model.context_window = 1_000_000;
        model
    }

    fn non_reasoning_model(id: &str) -> Model {
        let mut model = google_model(id);
        model.reasoning = false;
        model
    }

    fn user_message(text: &str) -> Message {
        Message::User(super::super::types::UserMessage {
            role: "user".to_string(),
            content: UserContent::text(text),
            timestamp: 0,
        })
    }

    fn assistant_message(
        id: &str,
        content: Vec<AssistantContent>,
        stop_reason: StopReason,
    ) -> Message {
        Message::Assistant(AssistantMessage {
            role: "assistant".to_string(),
            content,
            api: API_GOOGLE_GENERATIVE_AI.to_string(),
            provider: "google".to_string(),
            model: id.to_string(),
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

    fn tool_result(id: &str, name: &str, text: &str, is_error: bool) -> Message {
        Message::ToolResult(super::super::types::ToolResultMessage {
            role: "toolResult".to_string(),
            tool_call_id: id.to_string(),
            tool_name: name.to_string(),
            content: vec![TextOrImageContent::text(text)],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error,
            timestamp: 0,
        })
    }

    fn body_for(model: &Model, context: &Context) -> Value {
        build_request_body(model, context, None).unwrap()
    }

    // ── build_request_body ────────────────────────────────────────────

    #[test]
    fn body_includes_contents_system_instruction_and_generation_config() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: Some("You are helpful.".into()),
            messages: vec![user_message("hi")],
            tools: vec![Tool {
                name: "get_weather".into(),
                description: "Get weather".into(),
                parameters: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            }],
        };
        let body = body_for(&model, &context);

        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are helpful."
        );
        assert_eq!(
            body["tools"][0]["functionDeclarations"][0]["name"],
            "get_weather"
        );
        assert_eq!(
            body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]["type"],
            "object"
        );
        assert!(body["tools"][0]["functionDeclarations"][0]
            .get("parameters")
            .is_none());
        assert!(
            body.get("toolConfig").is_none(),
            "无 toolChoice 时不下发 toolConfig"
        );
        // maxOutputTokens 恒存在(下限 1)
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 8192);
        assert!(body.get("model").is_none(), "model 经 URL 承载");
    }

    #[test]
    fn body_carries_temperature_and_tool_config() {
        let model = google_model("gemini-2.5-flash");
        let mut options = SimpleStreamOptions::default();
        options.temperature = Some(0.5);
        options.tool_choice = Some(ToolChoice::Auto);
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![Tool {
                name: "t".into(),
                description: String::new(),
                parameters: json!({"type": "object"}),
            }],
        };
        let body = build_request_body(&model, &context, Some(&options)).unwrap();
        assert_eq!(body["generationConfig"]["temperature"], 0.5);
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    }

    #[test]
    fn tool_choice_none_maps_to_none_mode() {
        let model = google_model("gemini-2.5-flash");
        let mut options = SimpleStreamOptions::default();
        options.tool_choice = Some(ToolChoice::None);
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![Tool {
                name: "t".into(),
                description: String::new(),
                parameters: json!({"type": "object"}),
            }],
        };
        let body = build_request_body(&model, &context, Some(&options)).unwrap();
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "NONE");
    }

    #[test]
    fn non_reasoning_model_gets_no_thinking_config() {
        let model = non_reasoning_model("gemini-2.0-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        assert!(body["generationConfig"].get("thinkingConfig").is_none());
    }

    // ── thinking:Gemini 2 预算 / Gemini 3 档位 / 关闭思考 ─────────────

    #[test]
    fn gemini2_uses_thinking_budget_tables() {
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };

        let cases = [
            ("gemini-2.5-pro", ThinkingLevel::Medium, 8192i64),
            ("gemini-2.5-pro", ThinkingLevel::Minimal, 128),
            ("gemini-2.5-flash-lite", ThinkingLevel::Low, 2048),
            ("gemini-2.5-flash", ThinkingLevel::High, 24576),
            ("gemini-2.0-flash", ThinkingLevel::High, -1),
        ];
        for (id, level, budget) in cases {
            let model = google_model(id);
            let mut options = SimpleStreamOptions::default();
            options.reasoning = Some(level);
            let body = build_request_body(&model, &context, Some(&options)).unwrap();
            assert_eq!(
                body["generationConfig"]["thinkingConfig"]["thinkingBudget"], budget,
                "{id} {level:?}"
            );
            assert_eq!(
                body["generationConfig"]["thinkingConfig"]["includeThoughts"],
                true
            );
            assert!(body["generationConfig"]["thinkingConfig"]
                .get("thinkingLevel")
                .is_none());
        }
    }

    #[test]
    fn custom_thinking_budgets_override_defaults() {
        let model = google_model("gemini-2.5-flash");
        let mut options = SimpleStreamOptions::default();
        options.reasoning = Some(ThinkingLevel::High);
        options.thinking_budgets = Some(ThinkingBudgets {
            minimal: None,
            low: None,
            medium: None,
            high: Some(1234),
        });
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        let body = build_request_body(&model, &context, Some(&options)).unwrap();
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            1234
        );
    }

    #[test]
    fn gemini3_and_gemma4_use_thinking_levels() {
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        let cases = [
            ("gemini-3-pro-preview", ThinkingLevel::High, "HIGH"),
            ("gemini-3-pro-preview", ThinkingLevel::Minimal, "LOW"),
            ("gemini-3.1-pro", ThinkingLevel::Medium, "HIGH"),
            ("gemini-3-flash", ThinkingLevel::Medium, "MEDIUM"),
            ("gemini-flash-latest", ThinkingLevel::Minimal, "MINIMAL"),
            ("gemini-flash-lite-latest", ThinkingLevel::Low, "LOW"),
            ("gemma-4-27b", ThinkingLevel::Minimal, "MINIMAL"),
            ("gemma4-27b", ThinkingLevel::High, "HIGH"),
        ];
        for (id, level, expected) in cases {
            let model = google_model(id);
            let mut options = SimpleStreamOptions::default();
            options.reasoning = Some(level);
            let body = build_request_body(&model, &context, Some(&options)).unwrap();
            assert_eq!(
                body["generationConfig"]["thinkingConfig"]["thinkingLevel"], expected,
                "{id} {level:?}"
            );
            assert!(body["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none());
        }
    }

    #[test]
    fn disabled_thinking_degrades_by_model() {
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        // Gemini 2.x:budget = 0
        let body = body_for(&google_model("gemini-2.5-flash"), &context);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            0
        );
        // Gemini 3 Pro:最低可用的 LOW(不能完全关闭)
        let body = body_for(&google_model("gemini-3-pro"), &context);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "LOW"
        );
        // Gemini 3 Flash:MINIMAL
        let body = body_for(&google_model("gemini-3-flash"), &context);
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );
    }

    #[test]
    fn thinking_level_map_is_resolved_and_validated() {
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        // 合法映射:high → "minimal" 后走档位(需显式请求 reasoning 档位)
        let mut model = google_model("gemini-3-flash");
        model.thinking_level_map = Some(HashMap::from([(
            "high".to_string(),
            Some("minimal".to_string()),
        )]));
        let mut mapped_options = SimpleStreamOptions::default();
        mapped_options.reasoning = Some(ThinkingLevel::High);
        let body = build_request_body(&model, &context, Some(&mapped_options)).unwrap();
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
            "MINIMAL"
        );

        // 非法映射 → Err(编码为错误流)
        let mut model = google_model("gemini-3-flash");
        model.thinking_level_map = Some(HashMap::from([(
            "high".to_string(),
            Some("urgent".to_string()),
        )]));
        let mut options = SimpleStreamOptions::default();
        options.reasoning = Some(ThinkingLevel::High);
        let error = build_request_body(&model, &context, Some(&options)).unwrap_err();
        assert!(
            error.contains("Unsupported Google thinking level mapping for google/gemini-3-flash"),
            "{error}"
        );
    }

    // ── 消息转换 ──────────────────────────────────────────────────────

    #[test]
    fn user_images_become_inline_data() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![Message::User(super::super::types::UserMessage {
                role: "user".to_string(),
                content: UserContent::Blocks(vec![
                    TextOrImageContent::text("what is this?"),
                    TextOrImageContent::Image {
                        data: "aGVsbG8=".to_string(),
                        mime_type: "image/png".to_string(),
                    },
                ]),
                timestamp: 0,
            })],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "what is this?");
        assert_eq!(
            body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            body["contents"][0]["parts"][1]["inlineData"]["data"],
            "aGVsbG8="
        );
    }

    #[test]
    fn function_call_and_response_round_trip() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![
                user_message("weather?"),
                assistant_message(
                    "gemini-2.5-flash",
                    vec![
                        AssistantContent::text("checking"),
                        AssistantContent::ToolCall(ToolCall {
                            id: "call_1".into(),
                            name: "get_weather".into(),
                            arguments: serde_json::from_value(json!({"city": "Oslo"})).unwrap(),
                            thought_signature: Some("c2ln".into()),
                            namespace: None,
                        }),
                    ],
                    StopReason::ToolUse,
                ),
                tool_result("call_1", "get_weather", "18C", false),
            ],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        // assistant → role "model",functionCall 不带 id(Gemini 2)
        assert_eq!(body["contents"][1]["role"], "model");
        let call_part = &body["contents"][1]["parts"][1];
        assert_eq!(call_part["functionCall"]["name"], "get_weather");
        assert_eq!(call_part["functionCall"]["args"]["city"], "Oslo");
        assert!(call_part["functionCall"].get("id").is_none());
        assert_eq!(call_part["thoughtSignature"], "c2ln");
        // toolResult → user turn + functionResponse.output
        let response_part = &body["contents"][2]["parts"][0];
        assert_eq!(body["contents"][2]["role"], "user");
        assert_eq!(response_part["functionResponse"]["name"], "get_weather");
        assert_eq!(
            response_part["functionResponse"]["response"]["output"],
            "18C"
        );
        assert!(response_part["functionResponse"].get("id").is_none());
    }

    #[test]
    fn gemini3_requires_tool_call_ids_and_supports_multimodal_results() {
        let model = google_model("gemini-3-pro");
        let context = Context {
            system_prompt: None,
            messages: vec![
                // 跨模型回放:tool call id 会被归一(同模型历史保留原 id)
                assistant_message(
                    "gemini-2.5-flash",
                    vec![AssistantContent::ToolCall(ToolCall {
                        id: "call id with spaces".into(),
                        name: "pick".into(),
                        arguments: Map::new(),
                        thought_signature: None,
                        namespace: None,
                    })],
                    StopReason::ToolUse,
                ),
                Message::ToolResult(super::super::types::ToolResultMessage {
                    role: "toolResult".to_string(),
                    tool_call_id: "call id with spaces".into(),
                    tool_name: "pick".into(),
                    content: vec![
                        TextOrImageContent::text(""),
                        TextOrImageContent::Image {
                            data: "aGk=".into(),
                            mime_type: "image/jpeg".into(),
                        },
                    ],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    is_error: false,
                    timestamp: 0,
                }),
            ],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        let call_part = &body["contents"][0]["parts"][0];
        // id 消毒:非 [A-Za-z0-9_-] → _,截 64
        assert_eq!(call_part["functionCall"]["id"], "call_id_with_spaces");
        let response_part = &body["contents"][1]["parts"][0];
        assert_eq!(
            response_part["functionResponse"]["id"],
            "call_id_with_spaces"
        );
        // 无文本 → "(see attached image)";Gemini 3 图片内嵌 parts
        assert_eq!(
            response_part["functionResponse"]["response"]["output"],
            TOOL_RESULT_IMAGE_TEXT
        );
        assert_eq!(
            response_part["functionResponse"]["parts"][0]["inlineData"]["mimeType"],
            "image/jpeg"
        );
    }

    #[test]
    fn gemini2_tool_result_images_go_to_separate_turn() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![
                assistant_message(
                    "gemini-2.5-flash",
                    vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".into(),
                        name: "shot".into(),
                        arguments: Map::new(),
                        thought_signature: None,
                        namespace: None,
                    })],
                    StopReason::ToolUse,
                ),
                Message::ToolResult(super::super::types::ToolResultMessage {
                    role: "toolResult".to_string(),
                    tool_call_id: "call_1".into(),
                    tool_name: "shot".into(),
                    content: vec![TextOrImageContent::Image {
                        data: "aGk=".into(),
                        mime_type: "image/png".into(),
                    }],
                    details: None,
                    usage: None,
                    added_tool_names: None,
                    is_error: false,
                    timestamp: 0,
                }),
            ],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["response"]["output"],
            TOOL_RESULT_IMAGE_TEXT
        );
        assert!(body["contents"][1]["parts"][0]["functionResponse"]
            .get("parts")
            .is_none());
        // 独立 user turn
        assert_eq!(
            body["contents"][2]["parts"][0]["text"],
            TOOL_RESULT_IMAGE_TURN_TEXT
        );
        assert_eq!(
            body["contents"][2]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
    }

    #[test]
    fn consecutive_tool_results_merge_into_one_user_turn() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![
                assistant_message(
                    "gemini-2.5-flash",
                    vec![
                        AssistantContent::ToolCall(ToolCall {
                            id: "call_1".into(),
                            name: "a".into(),
                            arguments: Map::new(),
                            thought_signature: None,
                            namespace: None,
                        }),
                        AssistantContent::ToolCall(ToolCall {
                            id: "call_2".into(),
                            name: "b".into(),
                            arguments: Map::new(),
                            thought_signature: None,
                            namespace: None,
                        }),
                    ],
                    StopReason::ToolUse,
                ),
                tool_result("call_1", "a", "one", false),
                tool_result("call_2", "b", "two", false),
            ],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        // 两个 functionResponse 合并进同一个 user turn
        assert_eq!(body["contents"].as_array().unwrap().len(), 2);
        assert_eq!(body["contents"][1]["role"], "user");
        assert_eq!(body["contents"][1]["parts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn error_tool_result_uses_error_key_and_orphans_get_synthetic_results() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![
                assistant_message(
                    "gemini-2.5-flash",
                    vec![AssistantContent::ToolCall(ToolCall {
                        id: "call_1".into(),
                        name: "a".into(),
                        arguments: Map::new(),
                        thought_signature: None,
                        namespace: None,
                    })],
                    StopReason::ToolUse,
                ),
                user_message("stop"),
            ],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        // user 打断 → 孤儿 tool call 先补合成错误结果
        assert_eq!(
            body["contents"][1]["parts"][0]["functionResponse"]["response"]["error"],
            SYNTHETIC_TOOL_RESULT_TEXT
        );

        let error_context = Context {
            system_prompt: None,
            messages: vec![tool_result("call_1", "a", "boom", true)],
            tools: vec![],
        };
        let body = body_for(&model, &error_context);
        assert_eq!(
            body["contents"][0]["parts"][0]["functionResponse"]["response"]["error"],
            "boom"
        );
    }

    #[test]
    fn thought_signatures_replay_only_for_same_model_with_valid_base64() {
        let signature = "c2lnX25vbl9lbXB0eQ=="; // 合法 base64,4 对齐
        let same = vec![AssistantContent::Thinking {
            thinking: "pondering".into(),
            thinking_signature: Some(signature.into()),
            redacted: false,
        }];
        let context = Context {
            system_prompt: None,
            messages: vec![
                assistant_message("gemini-2.5-flash", same.clone(), StopReason::Stop),
                assistant_message("gemini-2.0-flash", same, StopReason::Stop),
            ],
            tools: vec![],
        };
        let model = google_model("gemini-2.5-flash");
        let body = body_for(&model, &context);
        // 同 provider/model:thought:true + 签名回放
        let same_part = &body["contents"][0]["parts"][0];
        assert_eq!(same_part["thought"], true);
        assert_eq!(same_part["thoughtSignature"], signature);
        // 跨模型:降级纯文本,无 thought / 签名
        let cross_part = &body["contents"][1]["parts"][0];
        assert!(cross_part.get("thought").is_none());
        assert!(cross_part.get("thoughtSignature").is_none());
        assert_eq!(cross_part["text"], "pondering");

        // 非法 base64(长度不齐 4)被丢弃
        let bad = vec![AssistantContent::Thinking {
            thinking: "x".into(),
            thinking_signature: Some("abc".into()),
            redacted: false,
        }];
        let context = Context {
            system_prompt: None,
            messages: vec![assistant_message("gemini-2.5-flash", bad, StopReason::Stop)],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        assert!(body["contents"][0]["parts"][0]
            .get("thoughtSignature")
            .is_none());
    }

    #[test]
    fn empty_text_block_with_signature_is_kept() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![assistant_message(
                "gemini-2.5-flash",
                vec![AssistantContent::Text {
                    text: String::new(),
                    text_signature: Some("c2ln".into()),
                }],
                StopReason::Stop,
            )],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        let part = &body["contents"][0]["parts"][0];
        assert_eq!(part["text"], "");
        assert_eq!(part["thoughtSignature"], "c2ln");
    }

    #[test]
    fn max_tokens_clamped_to_context_budget() {
        let mut model = google_model("gemini-2.5-flash");
        model.context_window = 100;
        model.max_tokens = 8192;
        let context = Context {
            system_prompt: None,
            messages: vec![user_message(&"0123456789".repeat(10))],
            tools: vec![],
        };
        let body = body_for(&model, &context);
        // available = 100 - 估算(约25) - 4096 兜底 → 收敛到下限 1
        let max_output = body["generationConfig"]["maxOutputTokens"]
            .as_i64()
            .unwrap();
        assert_eq!(max_output, 1, "上下文预算不足时收敛到下限");
    }

    // ── 工具 schema 清洗与辅助判定 ─────────────────────────────────────

    #[test]
    fn sanitize_for_open_api_strips_meta_declarations() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": {"x": {"type": "string"}},
            "definitions": {"y": {"type": "string"}},
            "type": "object",
            "properties": {
                "a": {"$comment": "note", "type": "string"},
                "b": {"anyOf": [{"$id": "z", "type": "null"}, {"type": "string"}]}
            }
        });
        let sanitized = sanitize_for_open_api(&schema);
        assert!(sanitized.get("$schema").is_none());
        assert!(sanitized.get("$defs").is_none());
        assert!(sanitized.get("definitions").is_none());
        assert_eq!(sanitized["type"], "object");
        assert!(sanitized["properties"]["a"].get("$comment").is_none());
        // 对齐蓝本:数组元素不递归,内部对象保留原样
        assert_eq!(sanitized["properties"]["b"]["anyOf"][0]["$id"], "z");
        assert_eq!(sanitized["properties"]["b"]["anyOf"][1]["type"], "string");
    }

    #[test]
    fn convert_tools_strips_meta_keys_on_legacy_parameters_path() {
        let tools = vec![Tool {
            name: "t".into(),
            description: "d".into(),
            parameters: json!({"$schema": "x", "type": "object"}),
        }];
        let converted = convert_tools(&tools, true, false).unwrap();
        assert!(converted[0]["functionDeclarations"][0]["parameters"]
            .get("$schema")
            .is_none());
    }

    #[test]
    fn convert_tools_returns_none_for_empty_list() {
        assert!(convert_tools(&[], false, true).is_none());
    }

    #[test]
    fn model_shape_detection_matches_blueprint() {
        assert_eq!(gemini_major_version("gemini-2.5-flash"), Some(2));
        assert_eq!(gemini_major_version("gemini-live-3-x"), Some(3));
        assert_eq!(gemini_major_version("GEMINI-3-PRO"), Some(3));
        assert_eq!(gemini_major_version("claude-4"), None);

        assert!(requires_tool_call_id("gemini-3-pro"));
        assert!(!requires_tool_call_id("gemini-2.5-flash"));
        assert!(requires_tool_call_id("claude-sonnet-4"));
        assert!(requires_tool_call_id("gpt-oss-20b"));

        assert!(supports_multimodal_function_response("gemini-3-pro"));
        assert!(!supports_multimodal_function_response("gemini-2.5-flash"));
        assert!(supports_multimodal_function_response("claude-4"));

        assert!(supports_google_strict_tool_sampling("gemini-3-flash"));
        assert!(!supports_google_strict_tool_sampling("gemini-2.5-pro"));

        assert!(is_gemma4_model("Gemma-4-27B"));
        assert!(is_gemma4_model("foo-gemma4-bar"));
        assert!(!is_gemma4_model("gemma-3-27b"));

        assert!(is_gemini_3_pro_model("gemini-3-pro-preview"));
        assert!(is_gemini_3_pro_model("gemini-3.1-pro"));
        assert!(!is_gemini_3_pro_model("gemini-3-flash"));
        assert!(is_gemini_3_flash_model("gemini-3.0-flash"));
        assert!(is_gemini_3_flash_model("gemini-flash-latest"));
        assert!(is_gemini_3_flash_model("gemini-flash-lite-latest"));
        assert!(!is_gemini_3_flash_model("gemini-2.5-flash"));
    }

    #[test]
    fn stop_reason_string_mapping() {
        assert_eq!(map_stop_reason_string("STOP"), StopReason::Stop);
        assert_eq!(map_stop_reason_string("MAX_TOKENS"), StopReason::Length);
        assert_eq!(map_stop_reason_string("SAFETY"), StopReason::Error);
        assert_eq!(
            map_stop_reason_string("MALFORMED_FUNCTION_CALL"),
            StopReason::Error
        );
        assert_eq!(map_stop_reason_string("WHATEVER"), StopReason::Error);
    }

    #[test]
    fn thought_signature_validation() {
        assert!(is_valid_thought_signature("c2ln"));
        assert!(is_valid_thought_signature("c2lnXw=="));
        assert!(!is_valid_thought_signature(""));
        assert!(!is_valid_thought_signature("abc"));
        assert!(!is_valid_thought_signature("sig=="));
        assert!(!is_valid_thought_signature("ab!c"));
        // retain:非空覆盖,空保持
        assert_eq!(
            retain_thought_signature(Some("old"), Some("new")),
            Some("new")
        );
        assert_eq!(retain_thought_signature(Some("old"), Some("")), Some("old"));
        assert_eq!(retain_thought_signature(None, None), None);
    }

    #[test]
    fn stream_url_respects_base_url_version_path() {
        let mut model = google_model("gemini-2.5-flash");
        // 空 baseUrl → 官方端点 + 默认 v1beta
        model.base_url = String::new();
        assert_eq!(
            build_stream_url(&model),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
        // 自定义 baseUrl 已含版本路径,不再追加 apiVersion(对齐 TS 注释)
        model.base_url = "https://proxy.example.com/v1beta/".into();
        assert_eq!(
            build_stream_url(&model),
            "https://proxy.example.com/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    // ── SSE 解码 ──────────────────────────────────────────────────────

    #[test]
    fn sse_decoder_handles_split_chunks_crlf_and_multi_data_lines() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"data: {\"a\"").is_empty());
        // 行在同一 chunk 内续完;事件以空行分发
        let events = decoder.push(b"1}\r\n\r\nevent: ping\r\ndata: first\ndata: second\r\n\r\n");
        assert_eq!(events, vec!["{\"a\"1}", "first\nsecond"]);
        // 未完结事件在缓冲中等待
        assert!(decoder.push(b"data: partial").is_empty());
    }

    #[test]
    fn sse_decoder_ignores_comments_and_other_fields() {
        let mut decoder = SseDecoder::default();
        let events = decoder.push(b": keep-alive\nid: 42\nevent: x\ndata: {\"ok\":true}\n\n");
        assert_eq!(events, vec!["{\"ok\":true}"]);
    }

    // ── 聚合器:chunk → 事件 ────────────────────────────────────────────

    fn apply_lines(
        aggregator: &mut StreamAggregator,
        lines: &[Value],
    ) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        for line in lines {
            events.extend(aggregator.apply_chunk(line));
        }
        events
    }

    #[test]
    fn aggregator_streams_text_thinking_and_tool_call_events() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);

        let events = apply_lines(
            &mut aggregator,
            &[
                json!({
                    "responseId": "resp-1",
                    "candidates": [{"content": {"parts": [{"text": "Hel", "thoughtSignature": "c2ln"}]}}],
                    "usageMetadata": {"promptTokenCount": 100, "cachedContentTokenCount": 20,
                        "candidatesTokenCount": 5, "thoughtsTokenCount": 0, "totalTokenCount": 105}
                }),
                json!({
                    "responseId": "resp-2",
                    "candidates": [{"content": {"parts": [{"text": "lo"}, {"text": "deep", "thought": true}]}}]
                }),
                json!({
                    "candidates": [{"content": {"parts": [
                        {"functionCall": {"name": "get_weather", "args": {"city": "Oslo"}, "id": "call_1"},
                         "thoughtSignature": "c2lnXzI="}
                    ]}, "finishReason": "STOP"}]
                }),
            ],
        );

        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                AssistantMessageEvent::TextStart { .. } => "text_start",
                AssistantMessageEvent::TextDelta { .. } => "text_delta",
                AssistantMessageEvent::TextEnd { .. } => "text_end",
                AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
                AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
                AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
                AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
                AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
                AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "text_start",
                "text_delta",
                "text_delta",
                "text_end",
                "thinking_start",
                "thinking_delta",
                "thinking_end",
                "toolcall_start",
                "toolcall_delta",
                "toolcall_end",
            ]
        );

        // 文本块签名保留自首个 delta
        let AssistantMessageEvent::TextEnd { content, .. } = &events[3] else {
            panic!("expected text_end");
        };
        assert_eq!(content, "Hello");

        // toolcall:完整参数 + 签名 + id 透传
        let AssistantMessageEvent::ToolcallEnd { tool_call, .. } = &events[9] else {
            panic!("expected toolcall_end");
        };
        assert_eq!(tool_call.id, "call_1");
        assert_eq!(tool_call.name, "get_weather");
        assert_eq!(tool_call.thought_signature.as_deref(), Some("c2lnXzI="));

        // finishReason STOP + 工具调用 → toolUse;usage 投影 + 成本
        let output = aggregator.output();
        assert_eq!(output.stop_reason, StopReason::ToolUse);
        assert_eq!(output.raw_stop_reason.as_deref(), Some("STOP"));
        assert_eq!(output.response_id.as_deref(), Some("resp-1"));
        assert_eq!(output.usage.input, 80);
        assert_eq!(output.usage.output, 5);
        assert_eq!(output.usage.cache_read, 20);
        assert_eq!(output.usage.total_tokens, 105);

        let (end_events, message, terminal) = aggregator.finalize(false, None);
        assert!(end_events.is_empty(), "打开块已在 functionCall 前收尾");
        assert_eq!(message.stop_reason, StopReason::ToolUse);
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::ToolUse,
                ..
            }
        ));
    }

    #[test]
    fn aggregator_regenerates_duplicate_tool_call_ids() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);
        let chunk = json!({
            "candidates": [{"content": {"parts": [
                {"functionCall": {"name": "a", "args": {}, "id": "dup"}},
                {"functionCall": {"name": "a", "args": {}, "id": "dup"}}
            ]}, "finishReason": "STOP"}]
        });
        let events = aggregator.apply_chunk(&chunk);
        let ids: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolcallEnd { tool_call, .. } => Some(tool_call.id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "dup");
        assert_ne!(ids[1], "dup");
        assert!(ids[1].starts_with("a_"));
    }

    #[test]
    fn aggregator_tool_call_without_id_gets_generated_id() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);
        let chunk = json!({
            "candidates": [{"content": {"parts": [
                {"functionCall": {"name": "search", "args": {"q": "x"}}}
            ]}}]
        });
        aggregator.apply_chunk(&chunk);
        let AssistantContent::ToolCall(tool_call) = &aggregator.output().content[0] else {
            panic!("expected tool call");
        };
        assert!(tool_call.id.starts_with("search_"));
        assert_eq!(tool_call.arguments["q"], "x");
    }

    #[test]
    fn aggregator_max_tokens_finish_reason_maps_to_length() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);
        aggregator.apply_chunk(&json!({
            "candidates": [{"content": {"parts": [{"text": "partial"}]}, "finishReason": "MAX_TOKENS"}]
        }));
        assert_eq!(aggregator.output().stop_reason, StopReason::Length);

        // 正常收尾:打开块补 end + done
        let (end_events, message, terminal) = aggregator.finalize(false, None);
        assert!(matches!(
            end_events[0],
            AssistantMessageEvent::TextEnd { .. }
        ));
        assert_eq!(message.stop_reason, StopReason::Length);
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Done {
                reason: StopReason::Length,
                ..
            }
        ));
    }

    #[test]
    fn finalize_without_finish_reason_is_an_error() {
        let model = google_model("gemini-2.5-flash");
        let aggregator = StreamAggregator::new(&model);
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Google stream ended without a finish reason")
        );
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Error {
                reason: StopReason::Error,
                ..
            }
        ));
    }

    #[test]
    fn finalize_with_provider_error_stop_reason_mentions_raw_reason() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);
        aggregator.apply_chunk(&json!({
            "candidates": [{"finishReason": "SAFETY"}]
        }));
        let (_, message, terminal) = aggregator.finalize(false, None);
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Provider stopped with: SAFETY")
        );
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Error {
                reason: StopReason::Error,
                ..
            }
        ));
    }

    #[test]
    fn finalize_aborted_after_block_end_events() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);
        aggregator.apply_chunk(&json!({
            "candidates": [{"content": {"parts": [{"text": "hi"}]}, "finishReason": "STOP"}]
        }));
        let (end_events, message, terminal) = aggregator.finalize(true, None);
        assert!(matches!(
            end_events[0],
            AssistantMessageEvent::TextEnd { .. }
        ));
        assert_eq!(message.stop_reason, StopReason::Aborted);
        assert_eq!(
            message.error_message.as_deref(),
            Some("Request was aborted")
        );
        assert!(matches!(
            terminal,
            AssistantMessageEvent::Error {
                reason: StopReason::Aborted,
                ..
            }
        ));
    }

    #[test]
    fn stream_error_skips_block_end_events() {
        let model = google_model("gemini-2.5-flash");
        let mut aggregator = StreamAggregator::new(&model);
        aggregator.apply_chunk(&json!({
            "candidates": [{"content": {"parts": [{"text": "hi"}]}}]
        }));
        let (end_events, message, terminal) =
            aggregator.finalize(false, Some("connection reset".to_string()));
        assert!(end_events.is_empty());
        assert_eq!(message.stop_reason, StopReason::Error);
        assert_eq!(message.error_message.as_deref(), Some("connection reset"));
        assert!(matches!(terminal, AssistantMessageEvent::Error { .. }));
    }

    // ── 流入口(非网络路径) ────────────────────────────────────────────

    #[tokio::test]
    async fn missing_api_key_emits_error_stream() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        let mut stream = stream_google_generative_ai(model, context, None, None);
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert_eq!(events.len(), 2, "start + error");
        assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
        match &events[1] {
            AssistantMessageEvent::Error { error, .. } => {
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("No API key for provider: google")
                );
            }
            other => panic!("expected error event, got {other:?}"),
        }
        let final_message = stream.result().await;
        assert_eq!(final_message.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn pre_cancelled_signal_aborts_before_request() {
        let model = google_model("gemini-2.5-flash");
        let context = Context {
            system_prompt: None,
            messages: vec![user_message("hi")],
            tools: vec![],
        };
        let signal = CancellationToken::new();
        signal.cancel();
        let mut options = SimpleStreamOptions::default();
        options.api_key = Some("key".into());
        let mut stream = stream_google_generative_ai(model, context, Some(options), Some(signal));
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event);
        }
        assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
        match &events[1] {
            AssistantMessageEvent::Error { reason, error, .. } => {
                assert_eq!(*reason, StopReason::Aborted);
                assert_eq!(error.error_message.as_deref(), Some("Request was aborted"));
            }
            other => panic!("expected aborted error event, got {other:?}"),
        }
    }

    // ── 重试延迟计算 ──────────────────────────────────────────────────

    #[test]
    fn retry_delay_honors_retry_after_headers() {
        use reqwest::header::HeaderMap;
        // retry-after-ms
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", "250".parse().unwrap());
        assert_eq!(
            compute_retry_delay(Some(&headers), 3, None, "boom").unwrap(),
            250
        );
        // retry-after 秒数
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "2".parse().unwrap());
        assert_eq!(
            compute_retry_delay(Some(&headers), 0, None, "boom").unwrap(),
            2000
        );
        // 超过上限(默认 60s)直接失败,消息携带 provider 错误
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "120".parse().unwrap());
        let error = compute_retry_delay(Some(&headers), 0, None, "429: slow down").unwrap_err();
        assert!(
            error.starts_with("Server requested 120s retry delay (max: 60s)"),
            "{error}"
        );
        assert!(error.ends_with("429: slow down"));
        // maxRetryDelayMs = 0 → 不限
        assert_eq!(
            compute_retry_delay(Some(&headers), 0, Some(0), "boom").unwrap(),
            120_000
        );
    }

    #[test]
    fn exponential_retry_delay_stays_bounded() {
        for retry_index in 0u32..6 {
            for _ in 0..8 {
                let delay = compute_retry_delay(None, retry_index, None, "boom").unwrap();
                let ceiling = (0.5 * 2f64.powi(retry_index as i32)).min(8.0) * 1000.0;
                assert!(delay as f64 <= ceiling, "{retry_index} {delay}");
                assert!(delay as f64 >= ceiling * 0.75, "{retry_index} {delay}");
            }
        }
    }
}
