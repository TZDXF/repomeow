use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;
use crate::agent::llm::retry::{is_retryable_assistant_error, retry_delay_ms, sleep_with_cancel, DEFAULT_BASE_DELAY_MS};
use crate::agent::llm::{AssistantMessage, StopReason};
use crate::agent::types::{AgentMessage, TypedMessage};
use crate::agent::Agent;
use super::*;

/// 项目问答回合的自动重试次数上限(provider/transport 瞬态错误,指数退避;
/// 蓝本 pi 默认 3 次,见 agent::llm::retry::DEFAULT_MAX_RETRIES)。
pub(super) const CHAT_MAX_RETRIES: u32 = 10;

/// 问答层退避上限:蓝本指数退避无封顶,10 次重试下尾段等待过长,钳到 60 秒。
pub(super) const CHAT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// 对齐 pi coding-agent 的普通 assistant 自动重试编排。
pub(super) async fn run_chat_prompt_with_retries(
    agent: &Agent,
    prompt: AgentMessage,
    signal: &CancellationToken,
    on_event: &Channel<ChatEvent>,
) -> Result<(), String> {
    run_chat_prompt_with_policy(
        agent,
        prompt,
        signal,
        CHAT_MAX_RETRIES,
        DEFAULT_BASE_DELAY_MS,
        |event| {
            let _ = on_event.send(event);
        },
    )
    .await
}

pub(super) async fn run_chat_prompt_with_policy(
    agent: &Agent,
    prompt: AgentMessage,
    signal: &CancellationToken,
    max_retries: u32,
    base_delay_ms: u64,
    emit: impl Fn(ChatEvent),
) -> Result<(), String> {
    let mut retry_attempt = 0;
    agent.prompt(prompt).await?;

    loop {
        let Some(last_assistant) = last_assistant_message(agent) else {
            return Err("chat agent completed without an assistant message".to_string());
        };
        match last_assistant.stop_reason {
            StopReason::Aborted => return Ok(()),
            StopReason::Error => {}
            _ => return Ok(()),
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
        let delay_ms = retry_delay_ms(base_delay_ms, retry_attempt).min(CHAT_MAX_RETRY_DELAY_MS);
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

