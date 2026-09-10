//! 项目问答会话的自动压缩:对齐 pi `AgentSession._checkCompaction` /
//! `_runAutoCompaction` 的消息级实现(chat 无 harness 会话条目树,直接在
//! `Agent` 的扁平消息列表上工作;摘要生成复用 harness compaction 模块)。
//!
//! 与 pi 的对应关系:
//! - 阈值判定 → [`threshold_trigger_tokens`](pi `shouldCompact` + 防重触发守卫);
//! - 溢出恢复 → [`overflow_recovery`](pi case 1/2:`isContextOverflow` /
//!   `isRecoverableLength`,单次压缩重试);
//! - 执行体 → [`compact_chat_history`](pi `_runAutoCompaction` 默认摘要路径)。

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::AppHandle;

use crate::agent::agent_loop::reasoning_from_thinking_level;
use crate::agent::harness::compaction::compaction::{
    estimate_context_tokens, estimate_tokens, generate_summary_with_usage, should_compact,
    DEFAULT_COMPACTION_SETTINGS,
};
use crate::agent::harness::messages::{
    create_compaction_summary_message, CompactionSummaryMessage,
};
use crate::agent::types::AgentMessage;
use crate::agent::Agent;
use crate::time_util::now_ts_nanos;

use super::{ChatEvent, CancelCell};

/// 触发原因(对齐 pi `compaction_start` 事件的 reason)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChatCompactionReason {
    /// 上下文占用越过阈值(context_window - reserve_tokens)。
    Threshold,
    /// 上下文溢出/截断恢复。
    Overflow,
}

impl ChatCompactionReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ChatCompactionReason::Threshold => "threshold",
            ChatCompactionReason::Overflow => "overflow",
        }
    }
}

/// 压缩执行所需的会话上下文(chat_send 期间由会话槽位快照提供)。
pub(super) struct ChatCompactionContext {
    pub app: AppHandle,
    pub cancel_cell: CancelCell,
    /// 最近一次压缩的 summary 消息时间戳(防重触发守卫)。
    pub last_compaction_ts: Arc<Mutex<i64>>,
    /// 会话级上下文占用展示槽(压缩后立刻回落)。
    pub context_tokens: Arc<Mutex<i64>>,
}

/// 阈值判定(对齐 pi `shouldCompact` 调用点 + 压缩时间戳守卫):
/// 返回当前上下文 token 估算;未越阈值/不适用返回 None。
///
/// 防重触发:usage 来源消息早于最近一次压缩时,其 usage 反映压缩前的
/// 旧上下文,直接跳过(对齐 pi 对 `estimate.lastUsageIndex` 的校验),
/// 等下一条 assistant 消息带来新鲜 usage。
pub(super) fn threshold_trigger_tokens(
    messages: &[AgentMessage],
    context_window: i64,
    last_compaction_ts: i64,
) -> Option<i64> {
    let settings = DEFAULT_COMPACTION_SETTINGS;
    if !settings.enabled || context_window <= 0 {
        return None;
    }
    let estimate = estimate_context_tokens(messages);
    if let Some(index) = estimate.last_usage_index {
        if messages[index].timestamp() <= last_compaction_ts {
            return None;
        }
    }
    should_compact(estimate.tokens, context_window, &settings).then_some(estimate.tokens)
}

/// 消息级压缩切点(对齐 pi `findCutPoint` 的边界语义):
/// 从尾部累计 keep_recent_tokens,向后对齐到最近的 user 消息边界——
/// 不以 assistant/toolResult 开头,避免切断工具调用链(provider 会拒绝
/// 孤儿 toolResult);预算内没有 user 边界时向前找,尾部宁大勿断链。
/// 返回保留尾部的起始下标;0 表示没有可压缩的前缀。
pub(super) fn find_message_cut_point(messages: &[AgentMessage], keep_recent_tokens: i64) -> usize {
    let mut accumulated = 0i64;
    let mut index = messages.len();
    while index > 0 {
        index -= 1;
        accumulated += estimate_tokens(&messages[index]);
        if accumulated >= keep_recent_tokens {
            break;
        }
    }
    for (offset, message) in messages.iter().enumerate().skip(index) {
        if message.role_name() == "user" {
            return offset;
        }
    }
    for offset in (0..index).rev() {
        if messages[offset].role_name() == "user" {
            return offset;
        }
    }
    0
}

/// 执行消息级压缩(对齐 pi `_runAutoCompaction` 的共享默认摘要路径):
/// LLM 生成摘要后,历史替换为 [compactionSummary 消息 + 保留尾部]。
/// 摘要请求走会话的 stream_fn 工厂(配置热读),并跟随会话取消令牌。
pub(super) async fn compact_chat_history(
    agent: &Agent,
    ctx: &ChatCompactionContext,
    reason: ChatCompactionReason,
    emit: &(dyn Fn(ChatEvent) + Send + Sync),
) -> Result<(), String> {
    let settings = DEFAULT_COMPACTION_SETTINGS;
    let model = agent.model();
    let messages = agent.messages();

    // 上一次压缩的摘要作为 previous_summary 增量更新(对齐 pi)。
    let prev_index = messages
        .iter()
        .rposition(|message| message.role_name() == CompactionSummaryMessage::ROLE);
    let previous_summary = prev_index.and_then(|index| match &messages[index] {
        AgentMessage::Custom(map) => {
            serde_json::from_value::<CompactionSummaryMessage>(Value::Object(map.clone()))
                .ok()
                .map(|message| message.summary)
        }
        _ => None,
    });
    let compactable_start = prev_index.map(|index| index + 1).unwrap_or(0);
    let compactable = &messages[compactable_start..];
    let cut = find_message_cut_point(compactable, settings.keep_recent_tokens);
    if cut == 0 {
        // 没有可压缩的前缀(尾部即全部),跳过
        return Ok(());
    }
    let tokens_before = estimate_context_tokens(&messages).tokens;
    emit(ChatEvent::CompactionStart {
        reason: reason.as_str().to_string(),
    });

    // 摘要调用的上下文构成估算覆写会话展示槽无意义,用一次性槽。
    let stream_fn = super::stream::chat_stream_fn(
        ctx.app.clone(),
        ctx.cancel_cell.clone(),
        Arc::new(Mutex::new(None)),
    );
    let thinking = reasoning_from_thinking_level(agent.thinking_level());
    let result = generate_summary_with_usage(
        compactable[..cut].to_vec(),
        &stream_fn,
        &model,
        settings.reserve_tokens,
        None,
        previous_summary.as_deref(),
        thinking,
    )
    .await;

    match result {
        Ok(summary) => {
            let timestamp = now_ts_nanos() / 1_000_000;
            let mut next = Vec::with_capacity(messages.len() - compactable_start - cut + 1);
            next.push(create_compaction_summary_message(
                summary.text,
                tokens_before,
                timestamp,
            ));
            next.extend(messages[compactable_start + cut..].iter().cloned());
            // 压缩后尾部无新鲜 usage(保留的 assistant usage 是旧上下文口径),
            // 用启发式估算展示值;防重触发由 last_compaction_ts 守卫负责。
            let tokens_after: i64 = next.iter().map(estimate_tokens).sum();
            agent.set_messages(next);
            *ctx.last_compaction_ts.lock().unwrap() = timestamp;
            *ctx.context_tokens.lock().unwrap() = tokens_after;
            emit(ChatEvent::CompactionEnd {
                reason: reason.as_str().to_string(),
                tokens_before,
                tokens_after: Some(tokens_after),
            });
            Ok(())
        }
        Err(error) => {
            // 压缩失败不阻断会话:历史保持原样,仅通知 UI 收尾
            emit(ChatEvent::CompactionEnd {
                reason: reason.as_str().to_string(),
                tokens_before,
                tokens_after: None,
            });
            Err(error.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::test_assistant;
    use crate::agent::llm::types::{AssistantContent, StopReason, Usage};
    use crate::agent::types::TypedMessage;

    fn user(text: &str, timestamp: i64) -> AgentMessage {
        AgentMessage::user_text(text, timestamp)
    }

    fn assistant_text(text: &str, timestamp: i64) -> AgentMessage {
        let mut message = test_assistant(vec![AssistantContent::text(text)], StopReason::Stop);
        message.timestamp = timestamp;
        AgentMessage::Message(TypedMessage::Assistant(message))
    }

    #[test]
    fn cut_point_aligns_to_user_boundary() {
        // 每条约 4000 字符 ≈ 1000 token(估算口径 (chars+3)/4)。
        let block = "x".repeat(4000);
        let mut messages = Vec::new();
        for i in 0..10 {
            messages.push(user(&format!("q{i} {block}"), i));
            messages.push(assistant_text(&format!("a{i} {block}"), i));
        }
        let cut = find_message_cut_point(&messages, 3000);
        // 切点必须落在 user 消息上,尾部不超预算太多
        assert_eq!(messages[cut].role_name(), "user");
        assert!(cut > 0);
        let tail: i64 = messages[cut..].iter().map(estimate_tokens).sum();
        assert!(tail <= 3000 + 1100);
    }

    #[test]
    fn cut_point_zero_when_nothing_to_cut() {
        let messages = vec![user("hi", 0), assistant_text("hello", 1)];
        assert_eq!(find_message_cut_point(&messages, 20000), 0);
    }

    #[test]
    fn cut_point_prefers_earlier_user_over_broken_tail() {
        // 预算内没有 user 边界(唯一的 user 在开头):对齐到 0 → 无可压缩前缀。
        let block = "x".repeat(4000);
        let mut messages = vec![user("q0", 0)];
        for i in 0..8 {
            messages.push(assistant_text(&format!("a{i} {block}"), i + 1));
        }
        assert_eq!(find_message_cut_point(&messages, 1000), 0);
    }

    #[test]
    fn threshold_guard_skips_stale_usage() {
        // 压缩后保留的 assistant 带着旧 usage(时间戳早于压缩点)
        let summary = create_compaction_summary_message("摘要", 150_000, 100);
        let mut stale = test_assistant(vec![AssistantContent::text("旧回答")], StopReason::Stop);
        stale.usage = Usage {
            input: 150_000,
            output: 10,
            total_tokens: 150_010,
            ..Usage::zero()
        };
        stale.timestamp = 50;
        let messages = vec![summary, AgentMessage::Message(TypedMessage::Assistant(stale))];
        // 旧 usage 不可信 → 不触发(防压缩后立即重触发)
        assert_eq!(threshold_trigger_tokens(&messages, 128_000, 100), None);
        // 无守卫时 150k > 128k-16k 会触发
        assert!(threshold_trigger_tokens(&messages, 128_000, 0).is_some());
    }
}
