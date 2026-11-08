use std::sync::{Arc, Mutex};

use super::session::*;
use super::stream::*;
use super::turn::*;
use super::*;
use crate::agent::llm::AssistantContent;
use crate::agent::llm::ModelThinkingLevel;
use crate::agent::Agent;

use std::collections::HashSet;

use tokio_util::sync::CancellationToken;

use crate::agent::llm::event_stream::event_stream;
use crate::agent::llm::{
    AssistantMessage, AssistantMessageEvent, Model, SimpleStreamOptions, StopReason, Usage,
    API_OPENAI_COMPLETIONS,
};
use crate::agent::types::{
    AgentLoopConfig, AgentMessage, AgentState, StreamFn, ToolExecutionMode, TypedMessage,
};

type EventLog = Arc<Mutex<Vec<ChatEvent>>>;

fn ok_message(model: &Model, text: &str) -> AssistantMessage {
    AssistantMessage {
        role: "assistant".to_string(),
        content: vec![AssistantContent::text(text)],
        api: API_OPENAI_COMPLETIONS.to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        usage: Usage::zero(),
        stop_reason: StopReason::Stop,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        duration_ms: None,
        thinking_level: None,
        timestamp: 0,
    }
}

/// 按脚本逐次返回最终消息的 StreamFn:错误编码为 Error 事件,
/// 成功编码为 Done 事件(与真实 provider 的流终态一致)。
fn scripted_stream_fn(script: Arc<Mutex<Vec<AssistantMessage>>>) -> StreamFn {
    Arc::new(move |_model, _context, _options| {
        let script = script.clone();
        Box::pin(async move {
            let final_message = script.lock().unwrap().remove(0);
            let (stream, writer) = event_stream::<AssistantMessageEvent, AssistantMessage>();
            writer.push(AssistantMessageEvent::Start {
                partial: AssistantMessage {
                    stop_reason: StopReason::Pending,
                    ..final_message.clone()
                },
            });
            writer.push(if final_message.stop_reason == StopReason::Error {
                AssistantMessageEvent::Error {
                    reason: final_message.stop_reason.clone(),
                    error: final_message.clone(),
                }
            } else {
                AssistantMessageEvent::Done {
                    reason: final_message.stop_reason.clone(),
                    message: final_message.clone(),
                }
            });
            writer.end(final_message);
            stream
        })
    })
}

fn test_agent(script: Arc<Mutex<Vec<AssistantMessage>>>) -> Agent {
    let model = Model::from_settings("gpt-test", "http://localhost");
    let state = AgentState {
        system_prompt: String::new(),
        model: model.clone(),
        thinking_level: ModelThinkingLevel::Off,
        tools: Vec::new(),
        messages: Vec::new(),
        is_streaming: false,
        streaming_message: None,
        pending_tool_calls: HashSet::new(),
        error_message: None,
    };
    let loop_config = AgentLoopConfig {
        model,
        stream: SimpleStreamOptions::default(),
        convert_to_llm: default_convert_to_llm(),
        transform_context: None,
        get_api_key: None,
        finish_turn: None,
        prepare_next_turn: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        tool_execution: ToolExecutionMode::Parallel,
        before_tool_call: None,
        after_tool_call: None,
        prepare_request: None,
    };
    Agent::new(state, loop_config, scripted_stream_fn(script))
}

fn event_log() -> EventLog {
    Arc::new(Mutex::new(Vec::new()))
}

fn assert_last_stop(agent: &Agent, reason: StopReason) {
    match agent.messages().last() {
        Some(AgentMessage::Message(TypedMessage::Assistant(message))) => {
            assert_eq!(message.stop_reason, reason);
        }
        other => panic!("expected trailing assistant message, got {other:?}"),
    }
}

#[tokio::test]
async fn retries_transient_error_then_succeeds() {
    let model = Model::from_settings("gpt-test", "http://localhost");
    let script = Arc::new(Mutex::new(vec![
        error_assistant_message(&model, "429: rate limited"),
        ok_message(&model, "recovered"),
    ]));
    let agent = test_agent(script);
    let events = event_log();
    let signal = CancellationToken::new();

    let result = run_chat_prompt_with_policy(
        &agent,
        AgentMessage::user_text("hi", 0),
        &signal,
        3,
        1,
        None,
        {
            let events = events.clone();
            move |event: ChatEvent| events.lock().unwrap().push(event)
        },
    )
    .await;

    assert!(result.is_ok());
    let log = events.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert!(
        matches!(
            &log[0],
            ChatEvent::RetryScheduled {
                attempt: 1,
                max_attempts: 3,
                delay_ms: 1,
                ..
            }
        ),
        "unexpected first event: {log:?}"
    );
    assert!(
        matches!(
            &log[1],
            ChatEvent::RetryStarted {
                attempt: 1,
                max_attempts: 3
            }
        ),
        "unexpected second event: {log:?}"
    );
    // 失败 attempt 已从上下文剔除:只剩 user + 恢复后的 assistant。
    assert_eq!(agent.messages().len(), 2);
    assert_last_stop(&agent, StopReason::Stop);
}

#[tokio::test]
async fn gives_up_after_max_retries_and_keeps_failed_attempt() {
    let model = Model::from_settings("gpt-test", "http://localhost");
    let script = Arc::new(Mutex::new(vec![
        error_assistant_message(&model, "503 service unavailable"),
        error_assistant_message(&model, "503 service unavailable"),
        error_assistant_message(&model, "503 service unavailable"),
    ]));
    let agent = test_agent(script);
    let events = event_log();
    let signal = CancellationToken::new();

    let result = run_chat_prompt_with_policy(
        &agent,
        AgentMessage::user_text("hi", 0),
        &signal,
        2,
        1,
        None,
        {
            let events = events.clone();
            move |event: ChatEvent| events.lock().unwrap().push(event)
        },
    )
    .await;

    assert_eq!(result.err().as_deref(), Some("503 service unavailable"));
    let log = events.lock().unwrap();
    assert_eq!(log.len(), 4);
    assert!(matches!(
        &log[0],
        ChatEvent::RetryScheduled { attempt: 1, .. }
    ));
    assert!(matches!(
        &log[3],
        ChatEvent::RetryStarted {
            attempt: 2,
            max_attempts: 2
        }
    ));
    // 最终失败 attempt 留在会话历史(对齐 pi:keep in session for history)。
    assert_eq!(agent.messages().len(), 2);
    assert_last_stop(&agent, StopReason::Error);
}

#[tokio::test]
async fn non_retryable_error_fails_fast_without_events() {
    let model = Model::from_settings("gpt-test", "http://localhost");
    let script = Arc::new(Mutex::new(vec![error_assistant_message(
        &model,
        "429 insufficient_quota",
    )]));
    let agent = test_agent(script);
    let events = event_log();
    let signal = CancellationToken::new();

    let result = run_chat_prompt_with_policy(
        &agent,
        AgentMessage::user_text("hi", 0),
        &signal,
        3,
        1,
        None,
        {
            let events = events.clone();
            move |event: ChatEvent| events.lock().unwrap().push(event)
        },
    )
    .await;

    assert_eq!(result.err().as_deref(), Some("429 insufficient_quota"));
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(agent.messages().len(), 2);
}

#[tokio::test]
async fn cancel_during_backoff_skips_next_request() {
    let model = Model::from_settings("gpt-test", "http://localhost");
    let script = Arc::new(Mutex::new(vec![
        error_assistant_message(&model, "502 bad gateway"),
        ok_message(&model, "never reached"),
    ]));
    let agent = test_agent(script.clone());
    let events = event_log();
    let signal = CancellationToken::new();

    // RetryScheduled 到达即取消:模拟用户在退避等待中点「停止」。
    let result = run_chat_prompt_with_policy(
        &agent,
        AgentMessage::user_text("hi", 0),
        &signal,
        3,
        60_000,
        None,
        {
            let events = events.clone();
            let signal = signal.clone();
            move |event: ChatEvent| {
                if matches!(event, ChatEvent::RetryScheduled { .. }) {
                    signal.cancel();
                }
                events.lock().unwrap().push(event);
            }
        },
    )
    .await;

    assert!(result.is_ok());
    let log = events.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert!(matches!(
        &log[0],
        ChatEvent::RetryScheduled { attempt: 1, .. }
    ));
    // 退避被中断后不再发起下一次请求:成功响应仍未从脚本消费,失败 attempt 已剔除。
    assert_eq!(script.lock().unwrap().len(), 1);
    assert_eq!(agent.messages().len(), 1);
}

#[test]
fn truncate_last_user_turn_drops_the_last_turn() {
    let model = Model::from_settings("gpt-test", "http://localhost");
    let agent = test_agent(Arc::new(Mutex::new(Vec::new())));
    agent.set_messages(vec![
        AgentMessage::user_text("第一问", 0),
        AgentMessage::Message(TypedMessage::Assistant(ok_message(&model, "第一答"))),
        AgentMessage::user_text("第二问", 0),
        AgentMessage::Message(TypedMessage::Assistant(ok_message(&model, "第二答"))),
    ]);

    assert!(truncate_last_user_turn(&agent));
    let messages = agent.messages();
    assert_eq!(messages.len(), 2);
    assert!(matches!(
        messages[1],
        AgentMessage::Message(TypedMessage::Assistant(_))
    ));

    // 再截一次:第一轮也被移除,会话清空;空会话幂等返回 false。
    assert!(truncate_last_user_turn(&agent));
    assert!(agent.messages().is_empty());
    assert!(!truncate_last_user_turn(&agent));
}
