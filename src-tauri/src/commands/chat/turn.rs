use super::compaction::{
    compact_chat_history, threshold_trigger_tokens, ChatCompactionContext, ChatCompactionReason,
};
use super::*;
use crate::agent::llm::overflow::{is_context_overflow, is_recoverable_length};
use crate::agent::llm::retry::{
    is_retryable_assistant_error, retry_delay_ms, sleep_with_cancel, DEFAULT_BASE_DELAY_MS,
};
use crate::agent::llm::{AssistantMessage, StopReason};
use crate::agent::types::{AgentMessage, TypedMessage};
use crate::agent::Agent;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;

/// 项目问答回合的自动重试次数上限(provider/transport 瞬态错误,指数退避;
/// 蓝本 pi 默认 3 次,见 agent::llm::retry::DEFAULT_MAX_RETRIES)。
pub(super) const CHAT_MAX_RETRIES: u32 = 10;

/// 问答层退避上限:蓝本指数退避无封顶,10 次重试下尾段等待过长,钳到 60 秒。
pub(super) const CHAT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// 溢出恢复动作(对齐 pi `_checkCompaction` case 1 的分支语义)。
enum OverflowAction {
    /// 非溢出/不可恢复,落入普通错误重试或完成路径。
    NotApplicable,
    /// 已压缩并 continue_run,回到循环检查新的末条消息。
    Retried,
    /// 本回合就此收尾。
    Finish(Result<(), String>),
}

/// 对齐 pi coding-agent 的普通 assistant 自动重试 + 自动压缩编排
/// (pi `AgentSession.prompt` → `_checkCompaction`/`_runAutoCompaction`)。
pub(super) async fn run_chat_prompt_with_retries(
    agent: &Agent,
    prompt: AgentMessage,
    signal: &CancellationToken,
    compaction: &ChatCompactionContext,
    on_event: &Channel<ChatEvent>,
) -> Result<(), String> {
    run_chat_prompt_with_policy(
        agent,
        prompt,
        signal,
        CHAT_MAX_RETRIES,
        DEFAULT_BASE_DELAY_MS,
        Some(compaction),
        |event| {
            let _ = on_event.send(event);
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_chat_prompt_with_policy(
    agent: &Agent,
    prompt: AgentMessage,
    signal: &CancellationToken,
    max_retries: u32,
    base_delay_ms: u64,
    compaction: Option<&ChatCompactionContext>,
    emit: impl Fn(ChatEvent) + Send + Sync,
) -> Result<(), String> {
    // pre-prompt 阈值压缩(对齐 pi `prompt()` 的前置 `_checkCompaction`)
    if let Some(ctx) = compaction {
        let model = agent.model();
        let last_ts = *ctx.last_compaction_ts.lock().unwrap();
        if threshold_trigger_tokens(&agent.messages(), model.context_window, last_ts).is_some() {
            let _ = compact_chat_history(agent, ctx, ChatCompactionReason::Threshold, &emit).await;
        }
    }

    let mut retry_attempt = 0;
    // 溢出恢复(压缩 + 重试)每次用户提问只尝试一次(对齐 pi
    // `_overflowRecoveryAttempted`,防无限循环)。
    let mut overflow_recovery_attempted = false;
    agent.prompt(prompt).await?;

    loop {
        let Some(last_assistant) = last_assistant_message(agent) else {
            return Err("chat agent completed without an assistant message".to_string());
        };
        match last_assistant.stop_reason {
            StopReason::Aborted => return Ok(()),
            StopReason::Error | StopReason::Length => {
                if let Some(ctx) = compaction {
                    match maybe_recover_overflow(
                        agent,
                        ctx,
                        &last_assistant,
                        &mut overflow_recovery_attempted,
                        signal,
                        &emit,
                    )
                    .await
                    {
                        OverflowAction::Retried => continue,
                        OverflowAction::Finish(result) => return result,
                        OverflowAction::NotApplicable => {}
                    }
                }
                if last_assistant.stop_reason == StopReason::Length {
                    // 普通 length 截断(输出已达上限):按完成处理
                    return Ok(());
                }
                let detail = last_assistant
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string());
                if retry_attempt >= max_retries || !is_retryable_assistant_error(&last_assistant) {
                    return Err(detail);
                }

                retry_attempt += 1;
                remove_last_failed_assistant(agent);
                let delay_ms =
                    retry_delay_ms(base_delay_ms, retry_attempt).min(CHAT_MAX_RETRY_DELAY_MS);
                emit(ChatEvent::RetryScheduled {
                    attempt: retry_attempt,
                    max_attempts: max_retries,
                    delay_ms,
                    message: detail,
                });
                if !sleep_with_cancel(delay_ms, signal).await {
                    return Ok(());
                }
                emit(ChatEvent::RetryStarted {
                    attempt: retry_attempt,
                    max_attempts: max_retries,
                });
                agent.continue_run().await?;
            }
            _ => {
                if let Some(ctx) = compaction {
                    let model = agent.model();
                    let same_model = last_assistant.provider == model.provider
                        && last_assistant.model == model.id;
                    // 静默溢出(z.ai:stop 但 usage 超窗):压缩不重试(对齐 pi case 2)
                    if same_model && is_context_overflow(&last_assistant, model.context_window) {
                        let _ =
                            compact_chat_history(agent, ctx, ChatCompactionReason::Overflow, &emit)
                                .await;
                        return Ok(());
                    }
                    // run 末阈值压缩(对齐 pi case 3:压缩后继续,不重试)
                    let last_ts = *ctx.last_compaction_ts.lock().unwrap();
                    if threshold_trigger_tokens(&agent.messages(), model.context_window, last_ts)
                        .is_some()
                    {
                        let _ =
                            compact_chat_history(agent, ctx, ChatCompactionReason::Threshold, &emit)
                                .await;
                    }
                }
                return Ok(());
            }
        }
    }
}

/// 溢出/可恢复截断恢复(对齐 pi `_checkCompaction` case 1):
/// 命中且未尝试过时,移除失败的末尾 assistant、压缩、`continue_run` 重试一次。
async fn maybe_recover_overflow(
    agent: &Agent,
    ctx: &ChatCompactionContext,
    last_assistant: &AssistantMessage,
    overflow_recovery_attempted: &mut bool,
    signal: &CancellationToken,
    emit: &(impl Fn(ChatEvent) + Send + Sync),
) -> OverflowAction {
    let model = agent.model();
    // 模型已切换时不按旧模型的溢出报错触发(对齐 pi sameModel 守卫)
    let same_model =
        last_assistant.provider == model.provider && last_assistant.model == model.id;
    let overflow = same_model && is_context_overflow(last_assistant, model.context_window);
    let recoverable = same_model && is_recoverable_length(last_assistant, model.max_tokens);
    if !overflow && !recoverable {
        return OverflowAction::NotApplicable;
    }
    if *overflow_recovery_attempted {
        // 已尝试过一次:Length 按完成收尾,Error 落入普通重试/报错
        return if last_assistant.stop_reason == StopReason::Length {
            OverflowAction::Finish(Ok(()))
        } else {
            OverflowAction::NotApplicable
        };
    }
    *overflow_recovery_attempted = true;
    remove_last_incomplete_assistant(agent);
    if let Err(error) = compact_chat_history(agent, ctx, ChatCompactionReason::Overflow, emit).await
    {
        eprintln!("[chat] 溢出恢复压缩失败: {error}");
        return if last_assistant.stop_reason == StopReason::Length {
            OverflowAction::Finish(Ok(()))
        } else {
            // 压缩未发生:Error 仍按普通瞬态重试处理
            OverflowAction::NotApplicable
        };
    }
    if signal.is_cancelled() {
        return OverflowAction::Finish(Ok(()));
    }
    match agent.continue_run().await {
        Ok(()) => OverflowAction::Retried,
        Err(error) => OverflowAction::Finish(Err(error)),
    }
}

pub(super) fn last_assistant_message(agent: &Agent) -> Option<AssistantMessage> {
    match agent.messages().last() {
        Some(AgentMessage::Message(TypedMessage::Assistant(message))) => Some(message.clone()),
        _ => None,
    }
}

pub(super) fn remove_last_failed_assistant(agent: &Agent) {
    let mut messages = agent.messages();
    if matches!(
        messages.last(),
        Some(AgentMessage::Message(TypedMessage::Assistant(message)))
            if message.stop_reason == StopReason::Error
    ) {
        messages.pop();
        agent.set_messages(messages);
    }
}

/// 溢出恢复用:弹掉末尾 Error/Length 的 assistant(pi 语义:消息留在
/// session 历史但排除出重试上下文;chat 无 session 条目,直接移除)。
fn remove_last_incomplete_assistant(agent: &Agent) {
    let mut messages = agent.messages();
    if matches!(
        messages.last(),
        Some(AgentMessage::Message(TypedMessage::Assistant(message)))
            if matches!(message.stop_reason, StopReason::Error | StopReason::Length)
    ) {
        messages.pop();
        agent.set_messages(messages);
    }
}
