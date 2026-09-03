use std::sync::{Arc, Mutex};

use super::permission::*;
use super::session::*;
use super::stream::*;
use super::turn::*;
use super::*;
use crate::agent::llm::AssistantContent;
use crate::agent::llm::ModelThinkingLevel;
use crate::agent::types::BeforeToolCallContext;
use crate::agent::Agent;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent::llm::event_stream::event_stream;
use crate::agent::llm::{
    AssistantMessage, AssistantMessageEvent, Model, SimpleStreamOptions, StopReason, Usage,
    API_OPENAI_COMPLETIONS,
};
use crate::agent::types::{
    AgentLoopConfig, AgentMessage, AgentState, StreamFn, ToolExecutionMode, TypedMessage,
};
use crate::ai::catalog::{ChatPermission, ModelRef};

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
        should_stop_after_turn: None,
        prepare_next_turn: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        tool_execution: ToolExecutionMode::Parallel,
        before_tool_call: None,
        after_tool_call: None,
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

    let result =
        run_chat_prompt_with_policy(&agent, AgentMessage::user_text("hi", 0), &signal, 3, 1, {
            let events = events.clone();
            move |event: ChatEvent| events.lock().unwrap().push(event)
        })
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

    let result =
        run_chat_prompt_with_policy(&agent, AgentMessage::user_text("hi", 0), &signal, 2, 1, {
            let events = events.clone();
            move |event: ChatEvent| events.lock().unwrap().push(event)
        })
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

    let result =
        run_chat_prompt_with_policy(&agent, AgentMessage::user_text("hi", 0), &signal, 3, 1, {
            let events = events.clone();
            move |event: ChatEvent| events.lock().unwrap().push(event)
        })
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

// ── ask 权限工具硬确认 ─────────────────────────────────────────────

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

#[test]
fn gated_tool_list_is_exactly_the_five_side_effect_tools() {
    let mut tools = CONFIRM_REQUIRED_TOOLS.to_vec();
    tools.sort_unstable();
    assert_eq!(
        tools,
        vec![
            "add_custom_command",
            "generate_report",
            "regenerate_wiki",
            "set_wiki_model",
            "update_wiki"
        ]
    );
    for name in [
        "read_wiki",
        "list_custom_commands",
        "list_reports",
        "sem_find",
        "sem_context",
        "sem_relations",
        "sem_diff",
        "read_project_file",
        "get_ai_config",
    ] {
        assert!(
            !CONFIRM_REQUIRED_TOOLS.contains(&name),
            "{name} 不应在确认名单中"
        );
    }
}

#[tokio::test]
async fn permission_decision_covers_allow_deny_cancel_timeout_and_sender_drop() {
    // 允许
    let (sender, receiver) = oneshot::channel();
    sender.send(true).unwrap();
    assert_eq!(
        await_permission_decision(receiver, None, Duration::from_secs(5)).await,
        PermissionDecision::Allow
    );

    // 拒绝
    let (sender, receiver) = oneshot::channel();
    sender.send(false).unwrap();
    assert_eq!(
        await_permission_decision(receiver, None, Duration::from_secs(5)).await,
        PermissionDecision::Block(PERMISSION_DENIED_REASON)
    );

    // 取消:等待期间 abort 信号触发
    let signal = CancellationToken::new();
    let task_signal = signal.clone();
    let (_sender, receiver) = oneshot::channel();
    let waiter = tokio::spawn(async move {
        await_permission_decision(receiver, Some(&task_signal), Duration::from_secs(5)).await
    });
    signal.cancel();
    assert_eq!(
        waiter.await.unwrap(),
        PermissionDecision::Block(PERMISSION_CANCELLED_REASON)
    );

    // 发送端被丢弃(会话清理/登记被替换)→ 按取消处理,不死锁
    let (sender, receiver) = oneshot::channel();
    drop(sender);
    assert_eq!(
        await_permission_decision(receiver, None, Duration::from_secs(5)).await,
        PermissionDecision::Block(PERMISSION_CANCELLED_REASON)
    );

    // 超时(短超时,不真等 2 分钟)
    let (_sender, receiver) = oneshot::channel();
    let started = Instant::now();
    assert_eq!(
        await_permission_decision(receiver, None, Duration::from_millis(30)).await,
        PermissionDecision::Block(PERMISSION_TIMEOUT_REASON)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn deliver_permission_decision_is_idempotent_and_never_re_executes() {
    let pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // 未知 id:无副作用
    assert!(!deliver_permission_decision(&pending, "ghost", true));

    // 登记后首次响应消费通道;重复响应幂等失败,不会二次触发执行。
    let (sender, receiver) = oneshot::channel();
    pending.lock().unwrap().insert("call_1".to_string(), sender);
    assert!(deliver_permission_decision(&pending, "call_1", true));
    assert!(!deliver_permission_decision(&pending, "call_1", true));
    assert_eq!(receiver.await, Ok(true));
    assert!(pending.lock().unwrap().is_empty());

    // 决策已定(登记被钩子清除)后的迟到响应:无发送端,无副作用。
    assert!(!deliver_permission_decision(&pending, "call_1", false));
}

fn ask_prefs(permission: ChatPermission) -> Arc<Mutex<Option<ResolvedPrefs>>> {
    Arc::new(Mutex::new(Some(ResolvedPrefs {
        model_ref: ModelRef {
            provider_id: "test-provider".to_string(),
            model_id: "test-model".to_string(),
        },
        thinking: "off".to_string(),
        permission,
    })))
}

fn permission_context(id: &str, tool_name: &str) -> BeforeToolCallContext {
    let model = Model::from_settings("gpt-test", "http://localhost");
    BeforeToolCallContext {
        assistant_message: ok_message(&model, ""),
        tool_call: crate::agent::llm::ToolCall {
            id: id.to_string(),
            name: tool_name.to_string(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
            namespace: None,
        },
        args: serde_json::json!({}),
        context: crate::agent::types::AgentContext::default(),
    }
}

async fn wait_for_pending(
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    id: &str,
) {
    for _ in 0..200 {
        if pending.lock().unwrap().contains_key(id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("permission request {id} was never registered");
}

#[tokio::test]
async fn permission_hook_skips_non_ask_and_non_gated_tools() {
    // 权限 All:受控工具直接放行,不登记。
    let pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let hook = build_permission_hook(
        pending.clone(),
        ask_prefs(ChatPermission::All),
        Arc::new(Mutex::new(None)),
    );
    assert!(hook(permission_context("call_1", "update_wiki"), None)
        .await
        .is_none());
    assert!(hook(permission_context("call_2", "generate_report"), None)
        .await
        .is_none());
    assert!(pending.lock().unwrap().is_empty());

    // 权限 Ask:非受控工具直接放行。
    let hook = build_permission_hook(
        pending.clone(),
        ask_prefs(ChatPermission::Ask),
        Arc::new(Mutex::new(None)),
    );
    for name in [
        "read_wiki",
        "sem_find",
        "list_custom_commands",
        "read_project_file",
    ] {
        assert!(
            hook(permission_context("call_3", name), None)
                .await
                .is_none(),
            "{name} 不应被拦截"
        );
    }
    assert!(pending.lock().unwrap().is_empty());

    // 偏好快照缺失(异常兜底)同样直接放行。
    let hook = build_permission_hook(
        pending.clone(),
        Arc::new(Mutex::new(None)),
        Arc::new(Mutex::new(None)),
    );
    assert!(hook(permission_context("call_4", "update_wiki"), None)
        .await
        .is_none());
    assert!(pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn permission_hook_allows_and_denies_gated_tools() {
    let pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let hook = build_permission_hook(
        pending.clone(),
        ask_prefs(ChatPermission::Ask),
        Arc::new(Mutex::new(None)),
    );

    // 放行:决策 allow=true → 钩子返回 None(工具继续执行)。
    let allow_task = tokio::spawn({
        let hook = hook.clone();
        async move { hook(permission_context("call_1", "update_wiki"), None).await }
    });
    wait_for_pending(&pending, "call_1").await;
    assert!(deliver_permission_decision(&pending, "call_1", true));
    assert!(allow_task.await.unwrap().is_none());
    assert!(pending.lock().unwrap().is_empty());

    // 拒绝:决策 allow=false → block + 稳定英文理由。
    let deny_task = tokio::spawn({
        let hook = hook.clone();
        async move { hook(permission_context("call_2", "regenerate_wiki"), None).await }
    });
    wait_for_pending(&pending, "call_2").await;
    assert!(deliver_permission_decision(&pending, "call_2", false));
    let blocked = deny_task.await.unwrap().expect("拒绝应返回 block 结果");
    assert!(blocked.block);
    assert_eq!(blocked.reason.as_deref(), Some(PERMISSION_DENIED_REASON));
    assert!(!blocked.terminate);
    assert!(pending.lock().unwrap().is_empty());

    // 取消:abort 信号已取消 → block(cancelled),登记清理。
    let signal = CancellationToken::new();
    signal.cancel();
    let blocked = hook(
        permission_context("call_3", "add_custom_command"),
        Some(signal),
    )
    .await
    .expect("取消应返回 block 结果");
    assert!(blocked.block);
    assert_eq!(blocked.reason.as_deref(), Some(PERMISSION_CANCELLED_REASON));
    assert!(pending.lock().unwrap().is_empty());

    // 超时后迟到响应幂等:模拟超时路径清除登记后再响应,无执行。
    let (_sender, receiver) = oneshot::channel();
    assert_eq!(
        await_permission_decision(receiver, None, Duration::from_millis(20)).await,
        PermissionDecision::Block(PERMISSION_TIMEOUT_REASON)
    );
}
