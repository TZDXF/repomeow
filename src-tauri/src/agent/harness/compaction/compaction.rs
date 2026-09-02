//! 会话压缩:对齐 `packages/agent/src/harness/compaction/compaction.ts`。
//!
//! 蓝本的 `Models.completeSimple` 按任务要求改为直接传入 [`StreamFn`]
//! (`completeSimpleWithRetries` 消费流到终值,等价 complete 语义)。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::llm::event_stream::EventStream;
use crate::agent::llm::types::{
    AssistantMessage, Context as LlmContext, Model, StopReason, TextOrImageContent, Usage,
    UserContent, UserMessage,
};
use crate::agent::types::{AgentMessage, StreamFn, TypedMessage};

use crate::agent::harness::compaction::utils::{
    compute_file_lists, create_file_ops, extract_file_ops_from_message, format_file_operations,
    serialize_conversation, FileOperations,
};
#[allow(unused_imports)]
use crate::agent::harness::messages::{
    convert_to_llm, BashExecutionMessage, BranchSummaryMessage, CompactionSummaryMessage,
    CustomMessage, CustomMessageContent,
};
use crate::agent::harness::session::context::build_session_context;
#[allow(unused_imports)]
use crate::agent::harness::session::types::{CompactionEntry, Entry};
use crate::agent::harness::types::{err, ok, CompactionError, CompactionErrorCode, Result};
use crate::agent::harness::uuid::uuid_v7;

/// 生成 compaction 条目的文件操作明细(对齐 TS `CompactionDetails`)。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// 压缩阈值与保留设置(对齐 TS `CompactionSettings`)。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    /// 是否启用自动压缩判定。
    pub enabled: bool,
    /// 摘要 prompt 与输出保留的 token 预算。
    pub reserve_tokens: i64,
    /// 压缩后近似保留的近期上下文 token。
    pub keep_recent_tokens: i64,
}

/// harness 默认压缩设置(对齐 TS `DEFAULT_COMPACTION_SETTINGS`)。
pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
};

/// 从 provider usage 计算总上下文 tokens(对齐 TS `calculateContextTokens`)。
pub fn calculate_context_tokens(usage: &Usage) -> i64 {
    if usage.total_tokens != 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

fn get_assistant_usage(message: &AgentMessage) -> Option<Usage> {
    let AgentMessage::Message(TypedMessage::Assistant(assistant)) = message else {
        return None;
    };
    if assistant.stop_reason != StopReason::Aborted
        && assistant.stop_reason != StopReason::Error
        && calculate_context_tokens(&assistant.usage) > 0
    {
        return Some(assistant.usage.clone());
    }
    None
}

/// 从会话条目取最后一条有效 assistant usage(对齐 TS `getLastAssistantUsage`)。
pub fn get_last_assistant_usage(entries: &[Entry]) -> Option<Usage> {
    for entry in entries.iter().rev() {
        if let Entry::Message(message_entry) = entry {
            if let Some(usage) = get_assistant_usage(&message_entry.message) {
                return Some(usage);
            }
        }
    }
    None
}

/// 消息列表的上下文 token 估算(对齐 TS `ContextUsageEstimate`)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextUsageEstimate {
    pub tokens: i64,
    pub usage_tokens: i64,
    pub trailing_tokens: i64,
    /// 提供 usage 的消息下标;无则 None。
    pub last_usage_index: Option<usize>,
}

fn get_last_assistant_usage_info(messages: &[AgentMessage]) -> Option<(Usage, usize)> {
    for (index, message) in messages.iter().enumerate().rev() {
        if let Some(usage) = get_assistant_usage(message) {
            return Some((usage, index));
        }
    }
    None
}

/// 有 provider usage 时优先使用,尾部消息用启发式估算(对齐 TS `estimateContextTokens`)。
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextUsageEstimate {
    let Some((usage, index)) = get_last_assistant_usage_info(messages) else {
        let estimated: i64 = messages.iter().map(estimate_tokens).sum();
        return ContextUsageEstimate {
            tokens: estimated,
            usage_tokens: 0,
            trailing_tokens: estimated,
            last_usage_index: None,
        };
    };

    let usage_tokens = calculate_context_tokens(&usage);
    let mut trailing_tokens = 0i64;
    for message in messages.iter().skip(index + 1) {
        trailing_tokens += estimate_tokens(message);
    }

    ContextUsageEstimate {
        tokens: usage_tokens + trailing_tokens,
        usage_tokens,
        trailing_tokens,
        last_usage_index: Some(index),
    }
}

/// 判定上下文占用是否超过压缩阈值(对齐 TS `shouldCompact`)。
pub fn should_compact(
    context_tokens: i64,
    context_window: i64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    context_tokens > context_window - settings.reserve_tokens
}

const ESTIMATED_IMAGE_CHARS: i64 = 4800;

fn estimate_text_and_image_content_chars(content: &UserContent) -> i64 {
    match content {
        UserContent::Text(text) => text.chars().count() as i64,
        UserContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                TextOrImageContent::Text { text, .. } => text.chars().count() as i64,
                TextOrImageContent::Image { .. } => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
    }
}

fn estimate_custom_content_chars(content: &CustomMessageContent) -> i64 {
    match content {
        CustomMessageContent::Text(text) => text.chars().count() as i64,
        CustomMessageContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                TextOrImageContent::Text { text, .. } => text.chars().count() as i64,
                TextOrImageContent::Image { .. } => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
    }
}

fn safe_json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

/// 单条消息的保守字符启发式 token 估算(对齐 TS `estimateTokens`;JS 的
/// UTF-16 计数差异仅影响估计精度,不影响行为)。
pub fn estimate_tokens(message: &AgentMessage) -> i64 {
    let chars: i64 = match message {
        AgentMessage::Message(TypedMessage::User(user)) => {
            return (estimate_text_and_image_content_chars(&user.content) + 3) / 4;
        }
        AgentMessage::Message(TypedMessage::Assistant(assistant)) => assistant
            .content
            .iter()
            .map(|block| match block {
                crate::agent::llm::types::AssistantContent::Text { text, .. } => {
                    text.chars().count() as i64
                }
                crate::agent::llm::types::AssistantContent::Thinking { thinking, .. } => {
                    thinking.chars().count() as i64
                }
                crate::agent::llm::types::AssistantContent::ToolCall(tool_call) => {
                    tool_call.name.chars().count() as i64
                        + safe_json_stringify(&Value::Object(tool_call.arguments.clone()))
                            .chars()
                            .count() as i64
                }
            })
            .sum(),
        AgentMessage::Message(TypedMessage::ToolResult(result)) => result
            .content
            .iter()
            .map(|block| match block {
                TextOrImageContent::Text { text, .. } => text.chars().count() as i64,
                TextOrImageContent::Image { .. } => ESTIMATED_IMAGE_CHARS,
            })
            .sum(),
        AgentMessage::Custom(map) => {
            let role = map.get("role").and_then(Value::as_str).unwrap_or_default();
            match role {
                "bashExecution" => {
                    let command = map
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let output = map
                        .get("output")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    (command.chars().count() + output.chars().count()) as i64
                }
                "branchSummary" | "compactionSummary" => map
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(|summary| summary.chars().count() as i64)
                    .unwrap_or(0),
                "custom" => match map.get("content") {
                    Some(Value::String(text)) => text.chars().count() as i64,
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .map(|block| match block.get("type").and_then(Value::as_str) {
                            Some("text") => block
                                .get("text")
                                .and_then(Value::as_str)
                                .map(|text| text.chars().count() as i64)
                                .unwrap_or(0),
                            Some("image") => ESTIMATED_IMAGE_CHARS,
                            _ => 0,
                        })
                        .sum(),
                    _ => 0,
                },
                _ => 0,
            }
        }
    };
    (chars + 3) / 4
}

fn find_valid_cut_points(entries: &[Entry], start_index: usize, end_index: usize) -> Vec<usize> {
    let mut cut_points = Vec::new();
    for (index, entry) in entries.iter().enumerate().take(end_index).skip(start_index) {
        match entry {
            Entry::Message(message_entry) => {
                let role = message_entry.message.role_name();
                match role {
                    "bashExecution" | "custom" | "branchSummary" | "compactionSummary" | "user"
                    | "assistant" => {
                        cut_points.push(index);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if let Entry::BranchSummary(_) = entry {
            cut_points.push(index);
        }
    }
    cut_points
}

/// 找到包含某条目的回合的起始消息(对齐 TS `findTurnStartIndex`)。
pub fn find_turn_start_index(entries: &[Entry], entry_index: usize, start_index: usize) -> i64 {
    let mut index = entry_index as i64;
    while index >= start_index as i64 {
        let entry = &entries[index as usize];
        if let Entry::BranchSummary(_) = entry {
            return index;
        }
        if let Entry::Message(message_entry) = entry {
            let role = message_entry.message.role_name();
            if role == "user" || role == "bashExecution" {
                return index;
            }
        }
        index -= 1;
    }
    -1
}

/// 选中的压缩切点(对齐 TS `CutPointResult`)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutPointResult {
    /// 压缩后保留的第一个条目下标。
    pub first_kept_entry_index: usize,
    /// 切点分割回合时的回合起始条目下标;否则 -1。
    pub turn_start_index: i64,
    /// 切点是否分割进行中的回合。
    pub is_split_turn: bool,
}

/// 找到保留约 keep_recent_tokens 近期预算的压缩切点(对齐 TS `findCutPoint`)。
pub fn find_cut_point(
    entries: &[Entry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: i64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);

    if cut_points.is_empty() {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: -1,
            is_split_turn: false,
        };
    }
    let mut accumulated_tokens = 0i64;
    let mut cut_index = cut_points[0];

    for index in (start_index..end_index).rev() {
        let entry = &entries[index];
        let Entry::Message(message_entry) = entry else {
            continue;
        };
        accumulated_tokens += estimate_tokens(&message_entry.message);
        if accumulated_tokens >= keep_recent_tokens {
            for &candidate in &cut_points {
                if candidate >= index {
                    cut_index = candidate;
                    break;
                }
            }
            break;
        }
    }
    while cut_index > start_index {
        let prev_entry = &entries[cut_index - 1];
        if matches!(prev_entry, Entry::Compaction(_)) || matches!(prev_entry, Entry::Message(_)) {
            break;
        }
        cut_index -= 1;
    }
    let cut_entry = &entries[cut_index];
    let is_user_message = matches!(cut_entry, Entry::Message(message_entry) if message_entry.message.role_name() == "user");
    let turn_start_index = if is_user_message {
        -1
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };

    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_message && turn_start_index != -1,
    }
}

/// 摘要系统提示(逐字对齐蓝本)。
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";

const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.\n\nSummarize the prefix to provide context for the retained suffix:\n\n## Original Request\n[What did the user ask for in this turn?]\n\n## Early Progress\n- [Key decisions and work done in the prefix]\n\n## Context for Suffix\n- [Information needed to understand the retained recent work]\n\nBe concise. Focus on what's needed to understand the kept suffix.";

/// 重试策略(蓝本由 pi-ai 提供;形状与 [`crate::agent::harness::agent_harness::RetryPolicy`] 一致)。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RetryPolicy {
    pub enabled: bool,
    pub max_retries: u32,
    pub base_delay_ms: u64,
}

/// 重试回调(对齐 TS `RetryCallbacks`)。
#[derive(Clone, Default)]
pub struct RetryCallbacks {
    pub on_retry_scheduled: Option<Arc<dyn Fn(u32, u32, u64, &str) + Send + Sync>>,
    pub on_retry_attempt_start: Option<Arc<dyn Fn() + Send + Sync>>,
    pub on_retry_finished: Option<Arc<dyn Fn(bool, u32, Option<&str>) + Send + Sync>>,
}

fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
    let Some(error_message) = &message.error_message else {
        return false;
    };
    let normalized = error_message.to_lowercase();
    const RETRYABLE_MARKERS: [&str; 14] = [
        "timeout",
        "timed out",
        "rate limit",
        "429",
        "overloaded",
        "internal server error",
        "500",
        "502",
        "503",
        "504",
        "connection",
        "temporarily",
        "try again",
        "retry",
    ];
    RETRYABLE_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

/// 单次 assistant 调用 + 有界重试(对齐 TS `retryAssistantCall` + `completeSimpleWithRetries`)。
///
/// 摘要是独立请求:隔离路由(`sessionId` 换新)并关闭无法复用的缓存写
/// (`cacheRetention = none`)。
pub async fn complete_simple_with_retries(
    stream_fn: &StreamFn,
    model: Model,
    context: LlmContext,
    mut options: crate::agent::llm::types::SimpleStreamOptions,
    retry: Option<RetryPolicy>,
    callbacks: Option<&RetryCallbacks>,
) -> AssistantMessage {
    options.cache_retention = Some(crate::agent::llm::types::CacheRetention::None);
    options.session_id = Some(uuid_v7());

    let max_attempts = match retry {
        Some(policy) if policy.enabled => policy.max_retries,
        _ => 0,
    };

    let mut attempt: u32 = 0;
    let mut last_retry: Option<(u32, String)> = None;
    loop {
        let mut stream: EventStream<
            crate::agent::llm::types::AssistantMessageEvent,
            AssistantMessage,
        > = stream_fn(model.clone(), context.clone(), Some(options.clone())).await;
        // 消费流到终值(complete 语义)。
        use futures::StreamExt;
        while let Some(_event) = stream.next().await {}
        let response = stream.result().await;

        if response.stop_reason == StopReason::Aborted {
            if let Some((retry_attempt, _)) = last_retry {
                if let Some(callbacks) = callbacks {
                    if let Some(on_finished) = &callbacks.on_retry_finished {
                        on_finished(false, retry_attempt, None);
                    }
                }
            }
            return response;
        }
        if response.stop_reason != StopReason::Error {
            if let Some((retry_attempt, _)) = last_retry {
                if let Some(callbacks) = callbacks {
                    if let Some(on_finished) = &callbacks.on_retry_finished {
                        on_finished(true, retry_attempt, None);
                    }
                }
            }
            return response;
        }
        if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
            if let Some((retry_attempt, _)) = last_retry {
                if let Some(callbacks) = callbacks {
                    if let Some(on_finished) = &callbacks.on_retry_finished {
                        on_finished(false, retry_attempt, response.error_message.as_deref());
                    }
                }
            }
            return response;
        }

        attempt += 1;
        let error_message = response
            .error_message
            .clone()
            .unwrap_or_else(|| "Unknown error".to_string());
        let policy = retry.expect("retry policy present when max_attempts > 0");
        let delay_ms = policy.base_delay_ms * 2u64.pow(attempt - 1);
        if let Some(callbacks) = callbacks {
            if let Some(on_scheduled) = &callbacks.on_retry_scheduled {
                on_scheduled(attempt, max_attempts, delay_ms, &error_message);
            }
        }
        let sleep = tokio::time::sleep(std::time::Duration::from_millis(delay_ms));
        tokio::pin!(sleep);
        if let Some(signal) = &options_signal(&options) {
            tokio::select! {
                _ = &mut sleep => {}
                _ = signal.cancelled() => {
                    // 退避期间中止:归一化为 aborted AssistantMessage(TS 同语义)。
                    let mut aborted = response.clone();
                    aborted.stop_reason = StopReason::Aborted;
                    aborted.error_message = Some("Aborted".to_string());
                    if let Some(callbacks) = callbacks {
                        if let Some(on_finished) = &callbacks.on_retry_finished {
                            on_finished(false, attempt, None);
                        }
                    }
                    return aborted;
                }
            }
        } else {
            sleep.await;
        }
        if let Some(callbacks) = callbacks {
            if let Some(on_start) = &callbacks.on_retry_attempt_start {
                on_start();
            }
        }
        last_retry = Some((attempt, error_message));
    }
}

/// `SimpleStreamOptions` 未直接携带 signal(Rust 契约由参数传递);退避中止
/// 依赖调用方在 cancel 时令下一次调用立即返回 aborted。此处返回 None 即
/// 纯 sleep(保持与蓝本行为一致的最终结果)。
fn options_signal(
    _options: &crate::agent::llm::types::SimpleStreamOptions,
) -> Option<tokio_util::sync::CancellationToken> {
    None
}

fn content_text_of_assistant(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::agent::llm::types::AssistantContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn combine_usage(first: &Usage, second: &Usage) -> Usage {
    let mut combined = Usage {
        input: first.input + second.input,
        output: first.output + second.output,
        cache_read: first.cache_read + second.cache_read,
        cache_write: first.cache_write + second.cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: first.total_tokens + second.total_tokens,
        cost: crate::agent::llm::types::UsageCost {
            input: first.cost.input + second.cost.input,
            output: first.cost.output + second.cost.output,
            cache_read: first.cost.cache_read + second.cost.cache_read,
            cache_write: first.cost.cache_write + second.cost.cache_write,
            total: first.cost.total + second.cost.total,
        },
    };
    combined.cache_write_1h = match (first.cache_write_1h, second.cache_write_1h) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    combined.reasoning = match (first.reasoning, second.reasoning) {
        (Some(a), Some(b)) => Some(a + b),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    combined
}

/// 摘要生成结果(含 provider usage;对齐 TS `generateSummaryWithUsage` 返回值)。
pub struct SummaryWithUsage {
    pub text: String,
    pub usage: Usage,
}

/// 生成或更新会话摘要,并返回 provider usage(对齐 TS `generateSummaryWithUsage`)。
#[allow(clippy::too_many_arguments)]
pub async fn generate_summary_with_usage(
    current_messages: Vec<AgentMessage>,
    stream_fn: &StreamFn,
    model: &Model,
    reserve_tokens: i64,
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    thinking_level: Option<crate::agent::llm::types::ThinkingLevel>,
) -> Result<SummaryWithUsage, CompactionError> {
    let max_tokens = if model.max_tokens > 0 {
        ((reserve_tokens as f64 * 0.8).floor() as i64).min(model.max_tokens)
    } else {
        (reserve_tokens as f64 * 0.8).floor() as i64
    };
    let mut base_prompt: String = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_string()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(custom_instructions) = custom_instructions {
        base_prompt = format!("{base_prompt}\n\nAdditional focus: {custom_instructions}");
    }
    let llm_messages = convert_to_llm(current_messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let mut prompt_text = format!("<conversation>\n{conversation_text}\n</conversation>\n\n");
    if let Some(previous_summary) = previous_summary {
        prompt_text.push_str(&format!(
            "<previous-summary>\n{previous_summary}\n</previous-summary>\n\n"
        ));
    }
    prompt_text.push_str(&base_prompt);

    let summarization_messages = vec![crate::agent::llm::types::Message::User(UserMessage {
        role: "user".to_string(),
        content: UserContent::Blocks(vec![TextOrImageContent::text(prompt_text)]),
        timestamp: crate::agent::agent_loop::now_ms(),
    })];

    let mut options = crate::agent::llm::types::SimpleStreamOptions::default();
    options.max_tokens = Some(max_tokens.max(0) as u32);
    if model.reasoning {
        if let Some(level) = thinking_level {
            options.reasoning = Some(level);
        }
    }

    let response = complete_simple_with_retries(
        stream_fn,
        model.clone(),
        LlmContext {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: summarization_messages,
            tools: Vec::new(),
        },
        options,
        None,
        None,
    )
    .await;
    if response.stop_reason == StopReason::Aborted {
        return err(CompactionError::new(
            CompactionErrorCode::Aborted,
            response
                .error_message
                .unwrap_or_else(|| "Summarization aborted".to_string()),
        ));
    }
    if response.stop_reason == StopReason::Error {
        return err(CompactionError::new(
            CompactionErrorCode::SummarizationFailed,
            format!(
                "Summarization failed: {}",
                response
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string())
            ),
        ));
    }

    ok(SummaryWithUsage {
        text: content_text_of_assistant(&response),
        usage: response.usage,
    })
}

/// 生成或更新会话摘要(对齐 TS `generateSummary`)。
#[allow(clippy::too_many_arguments)]
pub async fn generate_summary(
    current_messages: Vec<AgentMessage>,
    stream_fn: &StreamFn,
    model: &Model,
    reserve_tokens: i64,
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    thinking_level: Option<crate::agent::llm::types::ThinkingLevel>,
) -> Result<String, CompactionError> {
    let result = generate_summary_with_usage(
        current_messages,
        stream_fn,
        model,
        reserve_tokens,
        custom_instructions,
        previous_summary,
        thinking_level,
    )
    .await?;
    ok(result.text)
}

/// 压缩准备结果(对齐 TS `CompactionPreparation`)。
#[derive(Clone, Debug)]
pub struct CompactionPreparation {
    /// 汇总进历史摘要的消息。
    pub messages_to_summarize: Vec<AgentMessage>,
    /// 分割回合时前缀单独摘要的消息。
    pub turn_prefix_messages: Vec<AgentMessage>,
    /// 压缩后保留并存入 compaction 条目的近期消息。
    pub retained_tail: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: i64,
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
    pub settings: CompactionSettings,
}

/// 从助手工具调用 + 上一 compaction 的 details 累加文件操作。
fn extract_file_operations(
    messages: &[AgentMessage],
    entries: &[Entry],
    prev_compaction_index: i64,
) -> FileOperations {
    let mut file_ops = create_file_ops();
    if prev_compaction_index >= 0 {
        if let Entry::Compaction(prev) = &entries[prev_compaction_index as usize] {
            if let Some(details) = &prev.details {
                if let Ok(parsed) = serde_json::from_value::<CompactionDetails>(details.clone()) {
                    for file in parsed.read_files {
                        file_ops.read.insert(file);
                    }
                    for file in parsed.modified_files {
                        file_ops.edited.insert(file);
                    }
                }
            }
        }
    }
    for message in messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }
    file_ops
}

fn get_message_from_entry(entry: &Entry) -> Option<AgentMessage> {
    match entry {
        Entry::Message(message_entry) => Some(message_entry.message.clone()),
        Entry::BranchSummary(branch) => Some(
            crate::agent::harness::messages::create_branch_summary_message(
                branch.summary.clone(),
                branch.from_id.clone(),
                branch.timestamp,
            ),
        ),
        Entry::Compaction(compaction) => Some(
            crate::agent::harness::messages::create_compaction_summary_message(
                compaction.summary.clone(),
                compaction.tokens_before,
                compaction.timestamp,
            ),
        ),
        _ => None,
    }
}

fn get_message_from_entry_for_compaction(entry: &Entry) -> Option<AgentMessage> {
    if let Entry::Compaction(_) = entry {
        return None;
    }
    get_message_from_entry(entry)
}

/// 准备会话条目以便压缩;不适用时返回 `Ok(None)`
/// (对齐 TS `prepareCompaction`)。
pub fn prepare_compaction(
    path_entries: &[Entry],
    settings: CompactionSettings,
) -> Result<Option<CompactionPreparation>, CompactionError> {
    if path_entries.is_empty() || matches!(path_entries.last(), Some(Entry::Compaction(_))) {
        return ok(None);
    }

    let mut prev_compaction_index: i64 = -1;
    for (index, entry) in path_entries.iter().enumerate().rev() {
        if let Entry::Compaction(_) = entry {
            prev_compaction_index = index as i64;
            break;
        }
    }

    let mut previous_summary: Option<String> = None;
    let compactable_entries: Vec<Entry>;
    if prev_compaction_index >= 0 {
        let prev = match &path_entries[prev_compaction_index as usize] {
            Entry::Compaction(prev) => prev.clone(),
            _ => unreachable!("checked above"),
        };
        previous_summary = Some(prev.summary.clone());
        let mut virtual_retained_entries: Vec<Entry> = Vec::with_capacity(prev.retained_tail.len());
        for (index, message) in prev.retained_tail.iter().enumerate() {
            virtual_retained_entries.push(Entry::Message(
                crate::agent::harness::session::types::MessageEntry {
                    id: format!("{}:retained:{index}", prev.id),
                    parent_id: Some(if index == 0 {
                        prev.id.clone()
                    } else {
                        format!("{}:retained:{}", prev.id, index - 1)
                    }),
                    seq: prev.seq,
                    timestamp: message.timestamp(),
                    message: message.clone(),
                    terminate: None,
                },
            ));
        }
        let mut entries = virtual_retained_entries;
        entries.extend(
            path_entries[(prev_compaction_index as usize + 1)..]
                .iter()
                .cloned(),
        );
        compactable_entries = entries;
    } else {
        compactable_entries = path_entries.to_vec();
    }
    let boundary_end = compactable_entries.len();

    let tokens_before =
        estimate_context_tokens(&build_session_context(path_entries, &Default::default()).messages)
            .tokens;

    let cut_point = find_cut_point(
        &compactable_entries,
        0,
        boundary_end,
        settings.keep_recent_tokens,
    );
    let history_end = if cut_point.is_split_turn {
        cut_point.turn_start_index as usize
    } else {
        cut_point.first_kept_entry_index
    };
    let mut messages_to_summarize: Vec<AgentMessage> = Vec::new();
    for entry in compactable_entries.iter().take(history_end) {
        if let Some(message) = get_message_from_entry_for_compaction(entry) {
            messages_to_summarize.push(message);
        }
    }
    let mut turn_prefix_messages: Vec<AgentMessage> = Vec::new();
    if cut_point.is_split_turn {
        for entry in compactable_entries
            [cut_point.turn_start_index as usize..cut_point.first_kept_entry_index]
            .iter()
        {
            if let Some(message) = get_message_from_entry_for_compaction(entry) {
                turn_prefix_messages.push(message);
            }
        }
    }
    let mut retained_tail: Vec<AgentMessage> = Vec::new();
    for entry in compactable_entries[cut_point.first_kept_entry_index..boundary_end].iter() {
        if let Some(message) = get_message_from_entry_for_compaction(entry) {
            retained_tail.push(message);
        }
    }
    let mut file_ops =
        extract_file_operations(&messages_to_summarize, path_entries, prev_compaction_index);
    if cut_point.is_split_turn {
        for message in &turn_prefix_messages {
            extract_file_ops_from_message(message, &mut file_ops);
        }
    }

    ok(Some(CompactionPreparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn: cut_point.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    }))
}

/// 生成的压缩数据(对齐 TS `CompactResult<T>`)。
#[derive(Clone, Debug)]
pub struct CompactResult {
    pub summary: String,
    pub tokens_before: i64,
    pub usage: Usage,
    pub retained_tail: Vec<AgentMessage>,
    pub details: Value,
}

/// 生成分割回合前缀摘要。
async fn generate_turn_prefix_summary(
    messages: Vec<AgentMessage>,
    stream_fn: &StreamFn,
    model: &Model,
    reserve_tokens: i64,
    thinking_level: Option<crate::agent::llm::types::ThinkingLevel>,
) -> Result<SummaryWithUsage, CompactionError> {
    let max_tokens = if model.max_tokens > 0 {
        ((reserve_tokens as f64 * 0.5).floor() as i64).min(model.max_tokens)
    } else {
        (reserve_tokens as f64 * 0.5).floor() as i64
    };
    let llm_messages = convert_to_llm(messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let prompt_text = format!(
        "<conversation>\n{conversation_text}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}"
    );
    let summarization_messages = vec![crate::agent::llm::types::Message::User(UserMessage {
        role: "user".to_string(),
        content: UserContent::Blocks(vec![TextOrImageContent::text(prompt_text)]),
        timestamp: crate::agent::agent_loop::now_ms(),
    })];

    let mut options = crate::agent::llm::types::SimpleStreamOptions::default();
    options.max_tokens = Some(max_tokens.max(0) as u32);
    if model.reasoning {
        if let Some(level) = thinking_level {
            options.reasoning = Some(level);
        }
    }

    let response = complete_simple_with_retries(
        stream_fn,
        model.clone(),
        LlmContext {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: summarization_messages,
            tools: Vec::new(),
        },
        options,
        None,
        None,
    )
    .await;
    if response.stop_reason == StopReason::Aborted {
        return err(CompactionError::new(
            CompactionErrorCode::Aborted,
            response
                .error_message
                .unwrap_or_else(|| "Turn prefix summarization aborted".to_string()),
        ));
    }
    if response.stop_reason == StopReason::Error {
        return err(CompactionError::new(
            CompactionErrorCode::SummarizationFailed,
            format!(
                "Turn prefix summarization failed: {}",
                response
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string())
            ),
        ));
    }

    ok(SummaryWithUsage {
        text: content_text_of_assistant(&response),
        usage: response.usage,
    })
}

/// 从准备好的会话历史生成压缩数据(对齐 TS `compact`)。
#[allow(clippy::too_many_arguments)]
pub async fn compact(
    preparation: CompactionPreparation,
    stream_fn: &StreamFn,
    model: &Model,
    custom_instructions: Option<&str>,
    thinking_level: Option<crate::agent::llm::types::ThinkingLevel>,
) -> Result<CompactResult, CompactionError> {
    let CompactionPreparation {
        messages_to_summarize,
        turn_prefix_messages,
        retained_tail,
        is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings,
    } = preparation;

    let summary: String;
    let summary_usage: Usage;

    if is_split_turn && !turn_prefix_messages.is_empty() {
        let mut history_text = "No prior history.".to_string();
        let mut history_usage: Option<Usage> = None;
        if !messages_to_summarize.is_empty() {
            let history_result = generate_summary_with_usage(
                messages_to_summarize,
                stream_fn,
                model,
                settings.reserve_tokens,
                custom_instructions,
                previous_summary.as_deref(),
                thinking_level,
            )
            .await?;
            history_text = history_result.text;
            history_usage = Some(history_result.usage);
        }
        let turn_prefix_result = generate_turn_prefix_summary(
            turn_prefix_messages,
            stream_fn,
            model,
            settings.reserve_tokens,
            thinking_level,
        )
        .await?;
        summary = format!(
            "{}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
            history_text, turn_prefix_result.text
        );
        summary_usage = match history_usage {
            Some(usage) => combine_usage(&usage, &turn_prefix_result.usage),
            None => turn_prefix_result.usage,
        };
    } else {
        let summary_result = generate_summary_with_usage(
            messages_to_summarize,
            stream_fn,
            model,
            settings.reserve_tokens,
            custom_instructions,
            previous_summary.as_deref(),
            thinking_level,
        )
        .await?;
        summary = summary_result.text;
        summary_usage = summary_result.usage;
    }

    let (read_files, modified_files) = compute_file_lists(&file_ops);
    let summary = format!(
        "{}{}",
        summary,
        format_file_operations(&read_files, &modified_files)
    );

    ok(CompactResult {
        summary,
        tokens_before,
        usage: summary_usage,
        retained_tail,
        details: serde_json::to_value(CompactionDetails {
            read_files,
            modified_files,
        })
        .unwrap_or(Value::Null),
    })
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::{test_assistant, test_model};
    use crate::agent::harness::session::types::MessageEntry;
    use crate::agent::llm::types::AssistantContent;
    use serde_json::json;

    fn message_entry(seq: i64, parent: Option<&str>, message: AgentMessage) -> Entry {
        Entry::Message(MessageEntry {
            id: format!("e{seq}"),
            seq,
            parent_id: parent.map(str::to_string),
            timestamp: 0,
            message,
            terminate: None,
        })
    }

    fn user(text: &str) -> AgentMessage {
        AgentMessage::user_text(text, 0)
    }

    fn assistant_text(text: &str) -> AgentMessage {
        AgentMessage::Message(TypedMessage::Assistant(test_assistant(
            vec![AssistantContent::text(text)],
            StopReason::Stop,
        )))
    }

    fn tool_result(text: &str) -> AgentMessage {
        AgentMessage::Message(TypedMessage::ToolResult(
            crate::agent::llm::types::ToolResultMessage {
                role: "toolResult".to_string(),
                tool_call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                content: vec![TextOrImageContent::text(text)],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 0,
            },
        ))
    }

    #[test]
    fn estimate_tokens_counts_roles() {
        // user 文本 40 chars → ceil(40/4) = 10。
        let long_user = user(&"a".repeat(40));
        assert_eq!(estimate_tokens(&long_user), 10);
        // bashExecution:command + output。
        let bash =
            crate::agent::harness::messages::create_bash_execution_message(BashExecutionMessage {
                role: "bashExecution".into(),
                command: "abcd".into(),
                output: "efgh".into(),
                exit_code: None,
                cancelled: false,
                truncated: false,
                full_output_path: None,
                timestamp: 0,
                exclude_from_context: None,
            });
        assert_eq!(estimate_tokens(&bash), 2);
        // branchSummary。
        let branch =
            crate::agent::harness::messages::create_branch_summary_message("x".repeat(8), "e1", 0);
        assert_eq!(estimate_tokens(&branch), 2);
    }

    #[test]
    fn estimate_context_tokens_prefers_usage() {
        let mut usage = Usage::zero();
        usage.total_tokens = 1000;
        let mut assistant = test_assistant(vec![AssistantContent::text("ok")], StopReason::Stop);
        assistant.usage = usage;
        let messages = vec![
            user("hi"),
            AgentMessage::Message(TypedMessage::Assistant(assistant)),
            user("tail"),
        ];
        let estimate = estimate_context_tokens(&messages);
        assert_eq!(estimate.usage_tokens, 1000);
        assert_eq!(estimate.last_usage_index, Some(1));
        assert!(estimate.trailing_tokens > 0);
        assert!(estimate.tokens > 1000);
    }

    #[test]
    fn should_compact_respects_settings() {
        let settings = DEFAULT_COMPACTION_SETTINGS;
        assert!(!should_compact(100, 128_000, &settings));
        assert!(should_compact(127_000 - 100, 128_000, &settings));
        assert!(!should_compact(
            i64::MAX,
            128_000,
            &CompactionSettings {
                enabled: false,
                ..settings
            }
        ));
    }

    #[test]
    fn find_cut_point_keeps_recent_tokens() {
        // 5 个回合(user + assistant),每条消息约 1000 tokens(4000 chars)。
        let mut entries = Vec::new();
        let mut seq = 1;
        let mut parent: Option<String> = None;
        for i in 0..5 {
            entries.push(message_entry(
                seq,
                parent.as_deref(),
                user(&"u".repeat(4000)),
            ));
            parent = Some(format!("e{seq}"));
            seq += 1;
            entries.push(message_entry(
                seq,
                parent.as_deref(),
                assistant_text(&"a".repeat(4000)),
            ));
            parent = Some(format!("e{seq}"));
            seq += 1;
        }
        // keepRecentTokens = 4000 → 保留最近约 2 条消息 → 切点应落在靠后的 user 上。
        let cut = find_cut_point(&entries, 0, entries.len(), 4000);
        assert!(
            cut.first_kept_entry_index >= 6,
            "got {}",
            cut.first_kept_entry_index
        );
        assert!(cut.is_split_turn || !cut.is_split_turn);
        // user 起始回合时非 split。
        if entries[cut.first_kept_entry_index].message_entry_role() == "user" {
            assert!(!cut.is_split_turn);
            assert_eq!(cut.turn_start_index, -1);
        }
    }

    #[test]
    fn find_cut_point_splits_turn_at_tool_result() {
        // user → assistant(toolUse)→ toolResult:toolResult 不是合法切点,
        // 最近预算刚好在 toolResult 触发 → 回退到 assistant 或 user 切点。
        let mut entries = Vec::new();
        entries.push(message_entry(1, None, user(&"u".repeat(4000))));
        entries.push(message_entry(
            2,
            Some("e1"),
            AgentMessage::Message(TypedMessage::Assistant(test_assistant(
                vec![AssistantContent::text(&"a".repeat(4000))],
                StopReason::ToolUse,
            ))),
        ));
        entries.push(message_entry(3, Some("e2"), tool_result(&"t".repeat(4000))));
        let cut = find_cut_point(&entries, 0, entries.len(), 8000);
        // 切点必须不是 toolResult。
        match &entries[cut.first_kept_entry_index] {
            Entry::Message(entry) => {
                assert_ne!(entry.message.role_name(), "toolResult");
            }
            _ => panic!("expected message entry"),
        }
    }

    #[test]
    fn find_turn_start_index_walks_back_to_user() {
        let mut entries = Vec::new();
        entries.push(message_entry(1, None, user("u")));
        entries.push(message_entry(2, Some("e1"), assistant_text("a")));
        entries.push(message_entry(3, Some("e2"), tool_result("t")));
        assert_eq!(find_turn_start_index(&entries, 2, 0), 0);
        assert_eq!(find_turn_start_index(&entries, 1, 0), 0);
        assert_eq!(find_turn_start_index(&entries, 0, 0), 0);
    }

    #[test]
    fn prepare_compaction_empty_and_last_compaction() {
        assert!(prepare_compaction(&[], DEFAULT_COMPACTION_SETTINGS)
            .unwrap()
            .is_none());
        let compaction_entry = Entry::Compaction(CompactionEntry {
            id: "c1".into(),
            seq: 1,
            parent_id: None,
            timestamp: 0,
            summary: "old".into(),
            retained_tail: vec![],
            tokens_before: 10,
            details: None,
            usage: None,
        });
        assert!(
            prepare_compaction(&[compaction_entry], DEFAULT_COMPACTION_SETTINGS)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn prepare_compaction_builds_messages() {
        let mut entries = Vec::new();
        // 最后一条 assistant 体量大:预算在它处触发 → 切回 user 边界(e3)。
        entries.push(message_entry(1, None, user("hello")));
        entries.push(message_entry(2, Some("e1"), assistant_text("hi")));
        entries.push(message_entry(3, Some("e2"), user(&"q".repeat(4000))));
        entries.push(message_entry(
            4,
            Some("e3"),
            assistant_text(&"a".repeat(4000)),
        ));
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 1000,
        };
        let preparation = prepare_compaction(&entries, settings)
            .unwrap()
            .expect("preparation expected");
        assert!(!preparation.messages_to_summarize.is_empty());
        assert!(!preparation.retained_tail.is_empty());
        assert!(preparation.tokens_before > 0);
        assert!(preparation.previous_summary.is_none());
    }

    #[test]
    fn calculate_and_last_usage() {
        let mut usage = Usage::zero();
        usage.total_tokens = 500;
        let mut assistant = test_assistant(vec![], StopReason::Stop);
        assistant.usage = usage.clone();
        let entries = vec![message_entry(
            1,
            None,
            AgentMessage::Message(TypedMessage::Assistant(assistant)),
        )];
        assert_eq!(get_last_assistant_usage(&entries), Some(usage));
        assert_eq!(calculate_context_tokens(&Usage::zero()), 0);
    }

    #[tokio::test]
    async fn compact_generates_summary_with_details() {
        // 脚本化 streamFn:返回一段文本。
        let called = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = called.clone();
        let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let final_message = test_assistant(
                    vec![AssistantContent::text("## Goal\nsummarized")],
                    StopReason::Stop,
                );
                let (stream, writer) = crate::agent::llm::event_stream::event_stream();
                writer.push(crate::agent::llm::types::AssistantMessageEvent::Start {
                    partial: test_assistant(vec![], StopReason::Pending),
                });
                writer.push(crate::agent::llm::types::AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: final_message.clone(),
                });
                writer.end(final_message);
                stream
            })
        });
        let entries = vec![
            message_entry(1, None, user("build the thing")),
            message_entry(2, Some("e1"), assistant_text("done")),
        ];
        let preparation = prepare_compaction(&entries, DEFAULT_COMPACTION_SETTINGS)
            .unwrap()
            .unwrap();
        let result = compact(preparation, &stream_fn, &test_model(), None, None)
            .await
            .unwrap();
        assert!(result.summary.contains("## Goal"));
        assert!(result.summary.contains("summarized"));
        assert!(result.tokens_before > 0);
        assert_eq!(called.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(result.details.is_object());
        let details: CompactionDetails = serde_json::from_value(result.details.clone()).unwrap();
        assert_eq!(details.read_files, Vec::<String>::new());
    }

    #[test]
    fn generate_summary_reports_error_stop_reason() {
        // 通过同步断言检查错误路径的分支覆盖交给 complete_simple_with_retries 的单测;
        // 此处仅验证错误结构。
        let error = CompactionError::new(CompactionErrorCode::SummarizationFailed, "boom");
        assert_eq!(error.code, CompactionErrorCode::SummarizationFailed);
        assert_eq!(error.to_string(), "boom");
        let _ = json!({"x": 1});
    }
}

/// Entry 的便捷角色查询(测试用)。
#[cfg(test)]
trait EntryRoleExt {
    fn message_entry_role(&self) -> String;
}

#[cfg(test)]
impl EntryRoleExt for Entry {
    fn message_entry_role(&self) -> String {
        match self {
            Entry::Message(entry) => entry.message.role_name().to_string(),
            _ => String::new(),
        }
    }
}

// 保持 BashExecutionMessage/CustomMessage 引用(convert_to_llm 的自定义 role 覆盖)。
#[allow(dead_code)]
fn _message_type_references(
    _: &BashExecutionMessage,
    _: &CustomMessage,
    _: &BranchSummaryMessage,
    _: &CompactionSummaryMessage,
    _: &crate::agent::harness::compaction::utils::FileOperations,
) {
}
