use std::sync::{Arc, Mutex};
use tauri::{AppHandle};
use tokio_util::sync::CancellationToken;
use crate::agent::llm::{AssistantMessage, AssistantMessageEvent, StopReason, Usage};
use crate::agent::types::{AgentEvent, AgentListener, AgentMessage, AgentToolResult, TextOrImageContent, TypedMessage};
use crate::ai::catalog::{self};
use crate::commands::usage::{insert_usage_row};
use crate::db::Db;
use crate::models::AiUsageRecord;
use crate::time_util::{now_ts};
use super::*;

// ── 事件监听与用量聚合 ───────────────────────────────────────────────

/// AgentEvent → ChatEvent 映射监听器:
/// - TextDelta → TextDelta;ThinkingDelta → ThinkingDelta(思考过程展示用)
/// - tool_execution_start/end → ToolCall / ToolResult
/// - MessageEnd(assistant)→ 累计 usage 并记录上下文占用
/// - 成功 TurnEnd → TurnEnd(附上下文构成估算;错误 attempt 由重试编排层处理,不向前端固化)
pub(super) fn chat_event_listener(
    usage: Arc<Mutex<Usage>>,
    context_tokens: Arc<Mutex<i64>>,
    breakdown: Arc<Mutex<Option<ChatContextBreakdown>>>,
    sink: EventSink,
) -> AgentListener {
    Arc::new(move |event: AgentEvent, _signal: CancellationToken| {
        // 闭包是 Fn(可能被多次调用),Arc 按次克隆进 async 块
        let usage = usage.clone();
        let context_tokens = context_tokens.clone();
        let breakdown = breakdown.clone();
        let sink = sink.clone();
        Box::pin(async move {
            match event {
                AgentEvent::MessageUpdate {
                    assistant_message_event,
                    ..
                } => match assistant_message_event {
                    AssistantMessageEvent::TextDelta { delta, .. } => {
                        sink_send(&sink, ChatEvent::TextDelta { delta });
                    }
                    AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                        sink_send(&sink, ChatEvent::ThinkingDelta { delta });
                    }
                    // toolcall_* 增量由工具执行事件表达,不透传
                    _ => {}
                },
                AgentEvent::ToolExecutionStart {
                    tool_call_id,
                    tool_name,
                    args,
                } => {
                    sink_send(
                        &sink,
                        ChatEvent::ToolCall {
                            id: tool_call_id,
                            name: tool_name,
                            args,
                        },
                    );
                }
                AgentEvent::ToolExecutionEnd {
                    tool_call_id,
                    result,
                    is_error,
                    ..
                } => {
                    sink_send(
                        &sink,
                        ChatEvent::ToolResult {
                            id: tool_call_id,
                            ok: !is_error,
                            summary: truncate_chars(&tool_result_text(&result), 300),
                        },
                    );
                }
                AgentEvent::MessageEnd { message } => {
                    if let AgentMessage::Message(TypedMessage::Assistant(assistant)) = message {
                        usage.lock().unwrap().add(&assistant.usage);
                        if assistant.usage.total_tokens > 0 {
                            *context_tokens.lock().unwrap() = assistant.usage.total_tokens;
                        }
                    }
                }
                AgentEvent::TurnEnd { message, .. } => {
                    let failed = matches!(
                        message,
                        AgentMessage::Message(TypedMessage::Assistant(AssistantMessage {
                            stop_reason: StopReason::Error | StopReason::Aborted,
                            ..
                        }))
                    );
                    if !failed {
                        let current = *context_tokens.lock().unwrap();
                        let breakdown = *breakdown.lock().unwrap();
                        sink_send(
                            &sink,
                            ChatEvent::TurnEnd {
                                context_tokens: (current > 0).then_some(current),
                                breakdown,
                            },
                        );
                    }
                }
                _ => {}
            }
        })
    })
}

pub(super) fn tool_result_text(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            TextOrImageContent::Text { text, .. } => Some(text.as_str()),
            TextOrImageContent::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}…")
}

pub(super) fn usage_to_summary(usage: &Usage, context_tokens: i64) -> ChatUsageSummary {
    let total = if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output
    };
    ChatUsageSummary {
        input_tokens: usage.input,
        output_tokens: usage.output,
        total_tokens: total,
        cached_tokens: (usage.cache_read > 0).then_some(usage.cache_read),
        cost_total: (usage.cost.total > 0.0).then_some(usage.cost.total),
        context_tokens: (context_tokens > 0).then_some(context_tokens),
    }
}

/// 聚合用量落库(task_type = "chat");token 列可空(计入调用次数)。
pub(super) fn record_chat_usage(db: &Db, app: &AppHandle, usage: &Usage, duration_ms: i64) {
    let model_id = catalog::resolve_chat_prefs(&catalog::load_ai_config_file(app))
        .map(|(reference, _)| reference.model_id)
        .unwrap_or_default();
    let record = AiUsageRecord {
        task_type: "chat".to_string(),
        model: model_id,
        input_tokens: (usage.input > 0).then_some(usage.input),
        output_tokens: (usage.output > 0).then_some(usage.output),
        total_tokens: (usage.total_tokens > 0).then_some(usage.total_tokens),
        duration_ms: Some(duration_ms),
        cached_tokens: (usage.cache_read > 0).then_some(usage.cache_read),
    };
    if let Ok(conn) = db.0.lock() {
        if let Err(error) = insert_usage_row(&conn, &record, now_ts()) {
            eprintln!("[chat] 记录 AI 用量失败: {error}");
        }
    }
}
