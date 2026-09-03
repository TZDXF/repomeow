//! 项目问答(chat)Tauri 命令层。
//!
//! 前端经 `chat_send` 发送消息,Rust 侧用 pi Agent(OpenAI 兼容流 + RepoMeow
//! 工具集)跑完整个对话回合,经 `Channel<ChatEvent>` 回推增量事件;会话按
//! 项目路径隔离,跨消息保留上下文。`chat_abort` 取消进行中的回合,
//! `chat_new_session` 丢弃会话上下文。每次 `chat_send` 结束后把聚合的
//! token 用量写入 `ai_usage_log`(task_type = "chat")。
//!
//! 模型/思考强度/工具权限来自 `ai-config.json` 的 `chat` 段(缺省回退
//! defaultModel),每次 `chat_send` 前重读:思考与权限变化经 `Agent` 的
//! 状态热切换方法就地生效(会话历史保留),模型与密钥由 StreamFn 每次
//! LLM 调用时重读。
//!
//! ask 权限(硬确认):工具集与 all 相同,但四个有副作用工具
//! (`update_wiki` / `regenerate_wiki` / `add_custom_command` /
//! `generate_report`)执行前经 `AgentLoopConfig.before_tool_call` 钩子拦截,
//! 推 `ToolPermissionRequest` 事件并等待 `chat_tool_permission_respond`
//! 决策(允许继续 / 拒绝或 2 分钟超时则 block);这些工具均带 sequential
//! 标记,含它们的批次整体顺序执行,确认一次最多挂起一个。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent::chat_tools::{chat_tools, ChatToolContext};
use crate::agent::llm::event_stream::event_stream;
use crate::agent::llm::openai_completions::stream_openai_completions;
use crate::agent::llm::retry::{
    is_retryable_assistant_error, retry_delay_ms, sleep_with_cancel, DEFAULT_BASE_DELAY_MS,
};
use crate::agent::llm::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Context, Model,
    SimpleStreamOptions, StopReason, Usage, API_OPENAI_COMPLETIONS,
};
use crate::agent::types::{
    AgentEvent, AgentListener, AgentLoopConfig, AgentMessage, AgentState, AgentToolResult,
    BeforeToolCallHookFn, BeforeToolCallResult, ConvertToLlmFn, Message, StreamFn,
    TextOrImageContent, ToolExecutionMode, TypedMessage,
};
use crate::agent::Agent;
use crate::ai::catalog::{self, ChatPermission, ModelRef};
use crate::commands::usage::{estimate_text_tokens, insert_usage_row};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::AiUsageRecord;
use crate::path_util::clean_str;
use crate::time_util::{now_ts, now_ts_nanos};

// ── 前端事件契约 ─────────────────────────────────────────────────────

/// 问答回合事件流(tag = "kind",字段 camelCase,与前端 chat 面板对齐)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChatEvent {
    /// assistant 正文增量。
    TextDelta { delta: String },
    /// assistant 思考增量(reasoning 模型;仅作展示,不进上下文统计)。
    ThinkingDelta { delta: String },
    /// 工具开始执行。
    ToolCall {
        id: String,
        name: String,
        args: Value,
    },
    /// 工具执行结束(summary 为结果文本摘要,截 300 字符)。
    ToolResult {
        id: String,
        ok: bool,
        summary: String,
    },
    /// ask 权限下,有副作用工具执行前等待用户确认(id 经
    /// `chat_tool_permission_respond` 回传决策;args 为校验后的参数)。
    ToolPermissionRequest {
        id: String,
        name: String,
        args: Value,
    },
    /// 一个回合(一次 LLM 调用 + 工具执行)结束,携带当前上下文占用
    /// (最近一条 assistant 消息的 usage.total_tokens,无数据为 null)
    /// 与最近一次请求的上下文构成估算。
    TurnEnd {
        context_tokens: Option<i64>,
        breakdown: Option<ChatContextBreakdown>,
    },
    /// 瞬态错误已进入退避等待(attempt 为 1-based 重试序号)。
    RetryScheduled {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        message: String,
    },
    /// 退避结束,即将开始下一次 assistant 调用。
    RetryStarted { attempt: u32, max_attempts: u32 },
    /// 回合正常结束,携带整个 prompt 的聚合用量。
    Done { usage: Option<ChatUsageSummary> },
    /// 失败/取消;code 取既有 ErrorCode 字符串。
    Error { code: String, message: String },
}

/// 一次 chat_send 的聚合 token 用量。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsageSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_tokens: Option<i64>,
    pub cost_total: Option<f64>,
    /// 当前上下文占用(最近 assistant 消息的 total_tokens)。
    pub context_tokens: Option<i64>,
}

/// 上下文构成估算(最近一次成功发起的 LLM 请求,按 system prompt / 工具定义 /
/// 消息三部分以 tiktoken 计量;本地估算口径,与 provider 实际计数存在出入)。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatContextBreakdown {
    pub system_prompt: i64,
    pub tools: i64,
    pub messages: i64,
}

// ── 会话与运行注册表 ─────────────────────────────────────────────────

/// 会话注册表:按 clean_str 后的项目路径隔离,一次只保留一份对话上下文。
static CHAT_SESSIONS: OnceLock<Mutex<HashMap<String, ChatSession>>> = OnceLock::new();

/// 运行注册表:run_id → (取消令牌, 会话键),供 chat_abort / chat_new_session
/// 取消进行中的回合。注意:commands/ai/run.rs 的 RegisteredRun 是 pub(super),
/// chat 模块无法复用,故本地等价实现;若主智能体把它提升为 pub(crate),
/// 可改为共用 AI_RUNS 让 ai_cancel_run 一并取消 chat 运行。
static CHAT_RUNS: OnceLock<Mutex<HashMap<String, (CancellationToken, String)>>> = OnceLock::new();

fn chat_sessions() -> &'static Mutex<HashMap<String, ChatSession>> {
    CHAT_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn chat_runs() -> &'static Mutex<HashMap<String, (CancellationToken, String)>> {
    CHAT_RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 每回合取消令牌的槽位:streamFn 在会话构建时捕获本槽,回合开始时写入
/// 新令牌(令牌不可重置,必须每回合换新),结束时清空。
#[derive(Clone, Default)]
struct CancelCell(Arc<Mutex<Option<CancellationToken>>>);

impl CancelCell {
    fn set(&self, token: CancellationToken) {
        *self.0.lock().unwrap() = Some(token);
    }

    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }

    fn get(&self) -> Option<CancellationToken> {
        self.0.lock().unwrap().clone()
    }
}

/// 事件汇:监听器在会话构建时捕获本槽,回合开始时放入本次 chat_send 的
/// Channel,结束时取出,保证事件始终发给当前调用方。
type EventSink = Arc<Mutex<Option<Channel<ChatEvent>>>>;

fn sink_send(sink: &EventSink, event: ChatEvent) {
    if let Some(channel) = sink.lock().unwrap().as_ref() {
        let _ = channel.send(event);
    }
}

/// 一个项目的问答会话(全部字段 Arc 化,可整体 Clone 快照)。
#[derive(Clone)]
struct ChatSession {
    agent: Arc<Agent>,
    cancel_cell: CancelCell,
    sink: EventSink,
    /// 监听器累计的本次回合用量(回合开始时清零)。
    usage: Arc<Mutex<Usage>>,
    /// 最近一条 assistant 消息的 total_tokens(上下文占用口径)。
    context_tokens: Arc<Mutex<i64>>,
    /// 最近一次 LLM 调用的上下文构成估算(streamFn 每次调用时刷新)。
    breakdown: Arc<Mutex<Option<ChatContextBreakdown>>>,
    busy: Arc<AtomicBool>,
    run_id: Arc<Mutex<String>>,
    /// 已解析的 chat 偏好快照(思考/权限/模型引用变化时热切换)。
    prefs: Arc<Mutex<Option<ResolvedPrefs>>>,
    /// ask 权限下待确认工具调用的一次性决策通道(tool_call_id → 发送端)。
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}

/// chat_send 期间注册在 CHAT_RUNS 的守卫,Drop 时移除。
struct RegisteredChatRun {
    id: String,
    token: CancellationToken,
}

impl RegisteredChatRun {
    fn new(id: String, session_key: String) -> Self {
        let token = CancellationToken::new();
        chat_runs()
            .lock()
            .unwrap()
            .insert(id.clone(), (token.clone(), session_key));
        Self { id, token }
    }
}

impl Drop for RegisteredChatRun {
    fn drop(&mut self) {
        chat_runs().lock().unwrap().remove(&self.id);
    }
}

/// 项目问答回合的自动重试次数上限(provider/transport 瞬态错误,指数退避;
/// 蓝本 pi 默认 3 次,见 agent::llm::retry::DEFAULT_MAX_RETRIES)。
const CHAT_MAX_RETRIES: u32 = 10;

/// 问答层退避上限:蓝本指数退避无封顶,10 次重试下尾段等待过长,钳到 60 秒。
const CHAT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// 对齐 pi coding-agent 的普通 assistant 自动重试编排。
async fn run_chat_prompt_with_retries(
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

async fn run_chat_prompt_with_policy(
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

fn last_assistant_message(agent: &Agent) -> Option<AssistantMessage> {
    match agent.messages().last() {
        Some(AgentMessage::Message(TypedMessage::Assistant(message))) => Some(message.clone()),
        _ => None,
    }
}

fn remove_last_failed_assistant(agent: &Agent) {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::agent::llm::AssistantContent;
    use crate::agent::llm::ModelThinkingLevel;
    use crate::agent::types::BeforeToolCallContext;
    use crate::agent::Agent;

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
    fn gated_tool_list_is_exactly_the_four_side_effect_tools() {
        let mut tools = CONFIRM_REQUIRED_TOOLS.to_vec();
        tools.sort_unstable();
        assert_eq!(
            tools,
            vec![
                "add_custom_command",
                "generate_report",
                "regenerate_wiki",
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
}

// ── Tauri 命令 ───────────────────────────────────────────────────────

/// 发送一条用户消息并跑完整个 agent 回合(可能多轮 LLM 调用 + 工具执行)。
/// 返回聚合用量;增量事件经 on_event 推送。
#[tauri::command]
pub async fn chat_send(
    app: AppHandle,
    db: State<'_, Db>,
    run_id: String,
    project_path: String,
    project_name: String,
    message: String,
    on_event: Channel<ChatEvent>,
) -> AppResult<Option<ChatUsageSummary>> {
    let session_key = clean_str(&project_path);
    let existing = chat_sessions().lock().unwrap().get(&session_key).cloned();
    let session = match existing {
        Some(session) => session,
        None => {
            let session = build_session(&app, &db, &project_path, &project_name)?;
            chat_sessions()
                .lock()
                .unwrap()
                .entry(session_key.clone())
                .or_insert(session)
                .clone()
        }
    };

    if session
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "chat_busy: agent is already processing",
        ));
    }

    // 热应用最新 chat 偏好(思考/权限/模型元数据),配置被清空则本条拒绝。
    if let Err(error) = apply_prefs(&app, &session) {
        session.busy.store(false, Ordering::SeqCst);
        return Err(error);
    }

    let started = Instant::now();
    let run = RegisteredChatRun::new(run_id.clone(), session_key);
    session.cancel_cell.set(run.token.clone());
    session.sink.lock().unwrap().replace(on_event.clone());
    *session.run_id.lock().unwrap() = run_id;
    *session.usage.lock().unwrap() = Usage::zero();

    let outcome = run_chat_prompt_with_retries(
        &session.agent,
        AgentMessage::user_text(message, now_ts_nanos() / 1_000_000),
        &run.token,
        &on_event,
    )
    .await;
    let usage_snapshot = session.usage.lock().unwrap().clone();
    let context_tokens = *session.context_tokens.lock().unwrap();
    let duration_ms = started.elapsed().as_millis() as i64;
    let cancelled = run.token.is_cancelled();
    drop(run);
    session.cancel_cell.clear();
    session.sink.lock().unwrap().take();
    // 回合结束:清空未决确认(正常路径钩子已自清,这里是兜底,杜绝迟到响应)。
    session.pending.lock().unwrap().clear();
    session.busy.store(false, Ordering::SeqCst);

    record_chat_usage(&db, &app, &usage_snapshot, duration_ms);
    let summary = usage_to_summary(&usage_snapshot, context_tokens);

    match (outcome, cancelled) {
        (Ok(_), false) => {
            let _ = on_event.send(ChatEvent::Done {
                usage: Some(summary.clone()),
            });
            Ok(Some(summary))
        }
        (_, true) => {
            // 中止:用量照常返回/落库(token 已消耗),前端以 Error 事件收尾。
            let _ = on_event.send(ChatEvent::Error {
                code: ErrorCode::AgentCanceled.as_str().to_string(),
                message: "chat aborted".to_string(),
            });
            Ok(Some(summary))
        }
        (Err(error), false) => {
            let detail = error.to_string();
            let _ = on_event.send(ChatEvent::Error {
                code: ErrorCode::AiRequestFailed.as_str().to_string(),
                message: detail.clone(),
            });
            Err(AppError::coded(ErrorCode::AiRequestFailed, detail))
        }
    }
}

/// 取消一次进行中的问答回合(run_id 为 chat_send 的入参)。幂等。
#[tauri::command]
pub async fn chat_abort(run_id: String) -> AppResult<()> {
    let registered = chat_runs().lock().unwrap().get(&run_id).cloned();
    if let Some((token, session_key)) = registered {
        token.cancel();
        let session = chat_sessions().lock().unwrap().get(&session_key).cloned();
        if let Some(session) = session {
            session.agent.abort();
            // 清空未决确认:等待中的钩子经 receiver Err 立即按取消收尾,不死锁。
            session.pending.lock().unwrap().clear();
        }
    }
    Ok(())
}

/// 回应 ask 权限下的工具执行确认(tool_call_id 来自 ToolPermissionRequest
/// 事件;allow=true 放行、false 拒绝)。未知 / 重复 / 已解决的请求幂等忽略,
/// 不会导致工具执行。
#[tauri::command]
pub async fn chat_tool_permission_respond(
    project_path: String,
    tool_call_id: String,
    allow: bool,
) -> AppResult<bool> {
    let session_key = clean_str(&project_path);
    let session = chat_sessions().lock().unwrap().get(&session_key).cloned();
    Ok(session
        .is_some_and(|session| deliver_permission_decision(&session.pending, &tool_call_id, allow)))
}

/// 丢弃某项目的会话上下文(下一条消息从零开始)。若该会话回合仍在跑,
/// 先取消再移除。
#[tauri::command]
pub async fn chat_new_session(project_path: String) -> AppResult<()> {
    let session_key = clean_str(&project_path);
    let removed = chat_sessions().lock().unwrap().remove(&session_key);
    if let Some(session) = removed {
        let run_id = session.run_id.lock().unwrap().clone();
        let token = chat_runs()
            .lock()
            .unwrap()
            .get(&run_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = token {
            token.cancel();
        }
        session.agent.abort();
        // 清空未决确认,避免钩子等待悬空。
        session.pending.lock().unwrap().clear();
    }
    Ok(())
}

// ── 会话构建 ─────────────────────────────────────────────────────────

/// ask 权限下执行前需用户硬确认的工具(有副作用:写入 wiki / 自定义命令 /
/// 生成报告)。
const CONFIRM_REQUIRED_TOOLS: [&str; 4] = [
    "update_wiki",
    "regenerate_wiki",
    "add_custom_command",
    "generate_report",
];

/// 确认等待的安全超时:超时按拒绝处理,避免会话永久挂起。
const PERMISSION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// 拒绝/超时/取消的稳定英文内部结果(作为 error 工具结果回传模型;
/// 前端用户文案走 i18n)。
const PERMISSION_DENIED_REASON: &str = "Tool execution was denied by the user";
const PERMISSION_TIMEOUT_REASON: &str = "Tool permission request timed out";
const PERMISSION_CANCELLED_REASON: &str = "Tool permission request was cancelled";

/// 一次工具确认的终局。
#[derive(Debug, PartialEq)]
enum PermissionDecision {
    /// 放行,继续执行工具。
    Allow,
    /// 拦截(block),携带稳定英文理由。
    Block(&'static str),
}

/// 等待工具确认决策:允许 / 拒绝 / 取消 / 超时(超时按拒绝)。一次性消费
/// receiver;决策到达前 sender 被丢弃(会话清理)视为取消。
async fn await_permission_decision(
    receiver: oneshot::Receiver<bool>,
    signal: Option<&CancellationToken>,
    timeout: Duration,
) -> PermissionDecision {
    let cancelled = async {
        match signal {
            Some(signal) => signal.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        result = receiver => match result {
            Ok(true) => PermissionDecision::Allow,
            Ok(false) => PermissionDecision::Block(PERMISSION_DENIED_REASON),
            Err(_) => PermissionDecision::Block(PERMISSION_CANCELLED_REASON),
        },
        _ = cancelled => PermissionDecision::Block(PERMISSION_CANCELLED_REASON),
        _ = tokio::time::sleep(timeout) => PermissionDecision::Block(PERMISSION_TIMEOUT_REASON),
    }
}

/// 投递一次工具确认决策。幂等:未知 id / 已解决 / 重复响应返回 false 且无
/// 任何副作用(绝不触发工具执行)。
fn deliver_permission_decision(
    pending: &Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    tool_call_id: &str,
    allow: bool,
) -> bool {
    let sender = pending.lock().unwrap().remove(tool_call_id);
    match sender {
        Some(sender) => sender.send(allow).is_ok(),
        None => false,
    }
}

/// ask 权限下的 before_tool_call 门禁:命中确认名单时登记一次性决策通道、
/// 推送 `ToolPermissionRequest` 并等待 `chat_tool_permission_respond` 决策。
/// 权限非 Ask 或工具不在名单时直接放行(返回 None)。通用 agent core 不改动。
fn build_permission_hook(
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    prefs: Arc<Mutex<Option<ResolvedPrefs>>>,
    sink: EventSink,
) -> BeforeToolCallHookFn {
    Arc::new(move |context, signal| {
        let pending = pending.clone();
        let prefs = prefs.clone();
        let sink = sink.clone();
        Box::pin(async move {
            let tool_name = context.tool_call.name.as_str();
            let permission = prefs.lock().unwrap().as_ref().map(|prefs| prefs.permission);
            if permission != Some(ChatPermission::Ask)
                || !CONFIRM_REQUIRED_TOOLS.contains(&tool_name)
            {
                return None;
            }
            let id = context.tool_call.id.clone();
            let args = context.args.clone();
            let (sender, receiver) = oneshot::channel();
            {
                let mut map = pending.lock().unwrap();
                // 同 id 重复登记(异常场景):替换旧发送端,旧决策立即失效。
                map.insert(id.clone(), sender);
            }
            sink_send(
                &sink,
                ChatEvent::ToolPermissionRequest {
                    id: id.clone(),
                    name: tool_name.to_string(),
                    args,
                },
            );
            let decision =
                await_permission_decision(receiver, signal.as_ref(), PERMISSION_WAIT_TIMEOUT).await;
            // 决策已定:清除登记,迟到/重复响应幂等失效,绝不导致执行。
            pending.lock().unwrap().remove(&id);
            match decision {
                PermissionDecision::Allow => None,
                PermissionDecision::Block(reason) => Some(BeforeToolCallResult {
                    block: true,
                    reason: Some(reason.to_string()),
                    terminate: false,
                }),
            }
        })
    })
}

/// 已解析的 chat 偏好快照(会话内缓存,变化才热切换)。
#[derive(Clone, Debug, PartialEq)]
struct ResolvedPrefs {
    model_ref: ModelRef,
    thinking: String,
    permission: ChatPermission,
}

/// 解析当前 chat 偏好 → (模型元数据, 快照, 厂商 api_key)。
/// 未配置/引用失效时返回 AiNotConfigured。
fn resolve_prefs(config_file: &catalog::AiConfigFile) -> AppResult<(Model, ResolvedPrefs, String)> {
    let Some((reference, prefs)) = catalog::resolve_chat_prefs(config_file) else {
        return Err(AppError::coded(ErrorCode::AiNotConfigured, ""));
    };
    let model = catalog::resolve_model(config_file, &reference.provider_id, &reference.model_id)?;
    let api_key = config_file
        .providers
        .get(&reference.provider_id)
        .map(|provider| provider.api_key.trim().to_string())
        .unwrap_or_default();
    if api_key.is_empty() {
        return Err(AppError::coded(ErrorCode::AiNotConfigured, ""));
    }
    Ok((
        model,
        ResolvedPrefs {
            model_ref: reference,
            thinking: prefs.thinking.clone(),
            permission: prefs.permission,
        },
        api_key,
    ))
}

/// 把最新 chat 偏好热应用到会话:思考变化就地换 AgentState(历史保留),
/// 模型元数据始终刷新;工具集与权限无关(All 与 Ask 均暴露全部工具,Ask 的
/// 确认在 before_tool_call 门禁层完成),故不再随权限重建。StreamFn 每次调用
/// 另行重读模型与密钥。
fn apply_prefs(app: &AppHandle, session: &ChatSession) -> AppResult<()> {
    let config_file = catalog::load_ai_config_file(app);
    let (model, resolved, _api_key) = resolve_prefs(&config_file)?;
    let previous = session.prefs.lock().unwrap().clone();
    if previous.as_ref() == Some(&resolved) {
        return Ok(());
    }
    if previous
        .as_ref()
        .is_none_or(|old| old.thinking != resolved.thinking)
    {
        session
            .agent
            .set_thinking_level(catalog::parse_thinking_level(&resolved.thinking));
    }
    session.agent.set_model(model);
    *session.prefs.lock().unwrap() = Some(resolved);
    Ok(())
}

fn build_session(
    app: &AppHandle,
    db: &Db,
    project_path: &str,
    project_name: &str,
) -> AppResult<ChatSession> {
    let config_file = catalog::load_ai_config_file(app);
    let (model, resolved, api_key) = resolve_prefs(&config_file)?;
    let context = ChatToolContext {
        project_path: project_path.to_string(),
        project_name: project_name.to_string(),
        project_id: lookup_project_id(db, project_path),
        worktree_path: None,
    };
    let state = AgentState {
        system_prompt: build_system_prompt(project_name, project_path),
        model: model.clone(),
        thinking_level: catalog::parse_thinking_level(&resolved.thinking),
        // All 与 Ask 均暴露全部工具;Ask 的确认在 before_tool_call 门禁完成。
        tools: chat_tools(app.clone(), context.clone()),
        messages: Vec::new(),
        is_streaming: false,
        streaming_message: None,
        pending_tool_calls: HashSet::new(),
        error_message: None,
    };
    let prefs_cell: Arc<Mutex<Option<ResolvedPrefs>>> = Arc::new(Mutex::new(Some(resolved)));
    let pending_cell: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let sink_cell: EventSink = Arc::new(Mutex::new(None));
    let loop_config = AgentLoopConfig {
        model: model.clone(),
        stream: SimpleStreamOptions {
            api_key: Some(api_key),
            ..Default::default()
        },
        convert_to_llm: default_convert_to_llm(),
        transform_context: None,
        get_api_key: None,
        should_stop_after_turn: None,
        prepare_next_turn: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        tool_execution: ToolExecutionMode::Parallel,
        // ask 权限下拦截四个有副作用工具的硬确认门禁(通用 agent core 不动)。
        before_tool_call: Some(build_permission_hook(
            pending_cell.clone(),
            prefs_cell.clone(),
            sink_cell.clone(),
        )),
        after_tool_call: None,
    };
    let cancel_cell = CancelCell::default();
    let breakdown_cell = Arc::new(Mutex::new(None));
    let agent = Arc::new(Agent::new(
        state,
        loop_config,
        chat_stream_fn(app.clone(), cancel_cell.clone(), breakdown_cell.clone()),
    ));
    let session = ChatSession {
        agent,
        cancel_cell,
        sink: sink_cell,
        usage: Arc::new(Mutex::new(Usage::zero())),
        context_tokens: Arc::new(Mutex::new(0)),
        breakdown: breakdown_cell,
        busy: Arc::new(AtomicBool::new(false)),
        run_id: Arc::new(Mutex::new(String::new())),
        prefs: prefs_cell,
        pending: pending_cell,
    };
    // 订阅一次,随会话存活;事件经 sink 槽转发给当前 chat_send 的 Channel。
    session.agent.subscribe(chat_event_listener(
        session.usage.clone(),
        session.context_tokens.clone(),
        session.breakdown.clone(),
        session.sink.clone(),
    ));
    Ok(session)
}

/// 系统提示:内置模板 + 项目上下文占位替换。
fn build_system_prompt(project_name: &str, project_path: &str) -> String {
    include_str!("../ai/prompts/chat-system.md")
        .replace("{{PROJECT_NAME}}", project_name)
        .replace("{{PROJECT_PATH}}", project_path)
}

/// projects 表按 path 查主键(未登记返回 None;路径按 clean_str 归一化)。
fn lookup_project_id(db: &Db, project_path: &str) -> Option<i64> {
    let conn = db.0.lock().ok()?;
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        [clean_str(project_path)],
        |row| row.get::<_, i64>(0),
    )
    .ok()
}

/// 对齐 agent.ts defaultConvertToLlm:已知 role 原样转换,Custom 全滤。
fn default_convert_to_llm() -> ConvertToLlmFn {
    Arc::new(|messages: Vec<AgentMessage>| {
        Box::pin(async move {
            messages
                .into_iter()
                .filter_map(|message| match message {
                    AgentMessage::Message(typed) => Some(match typed {
                        TypedMessage::User(user) => Message::User(user),
                        TypedMessage::Assistant(assistant) => Message::Assistant(assistant),
                        TypedMessage::ToolResult(result) => Message::ToolResult(result),
                    }),
                    AgentMessage::Custom(_) => None,
                })
                .collect::<Vec<_>>()
        })
    })
}

/// StreamFn 包装:每次 LLM 调用时重读 AI 配置(模型/密钥可热更新),
/// 并在发起请求前把上下文构成估算写入会话槽;配置缺失时按流契约把失败
/// 编码进事件流,绝不 panic。
fn chat_stream_fn(
    app: AppHandle,
    cancel_cell: CancelCell,
    breakdown: Arc<Mutex<Option<ChatContextBreakdown>>>,
) -> StreamFn {
    Arc::new(move |model, context, options| {
        let app = app.clone();
        let cancel_cell = cancel_cell.clone();
        let breakdown = breakdown.clone();
        let fallback_model = model;
        Box::pin(async move {
            let signal = cancel_cell.get();
            match load_stream_model(&app) {
                Ok((model, api_key)) => {
                    *breakdown.lock().unwrap() =
                        Some(estimate_context_breakdown(&model.id, &context));
                    let base = options.unwrap_or_default();
                    let options = SimpleStreamOptions {
                        api_key: Some(api_key),
                        ..base
                    };
                    stream_openai_completions(model, context, Some(options), signal)
                }
                Err(error) => error_event_stream(&fallback_model, &error.to_string()),
            }
        })
    })
}

/// 上下文构成估算:system prompt 按原文、工具定义与消息按 JSON 序列化文本
/// 计量(复用 ACP 用量兜底的 tiktoken 口径:已知模型选对应编码器,其余
/// 回退 o200k_base)。是占比展示用的近似值,不用于计费。
fn estimate_context_breakdown(model_id: &str, context: &Context) -> ChatContextBreakdown {
    let system_prompt = context
        .system_prompt
        .as_deref()
        .map(|text| estimate_text_tokens(model_id, text))
        .unwrap_or(0);
    let tools = context
        .tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(model_id, &serde_json::to_string(tool).unwrap_or_default())
        })
        .sum();
    let messages = context
        .messages
        .iter()
        .map(|message| {
            estimate_text_tokens(
                model_id,
                &serde_json::to_string(message).unwrap_or_default(),
            )
        })
        .sum();
    ChatContextBreakdown {
        system_prompt,
        tools,
        messages,
    }
}

/// 读取 AI 配置并解析 chat 偏好指向的模型;要求已配置。
fn load_stream_model(app: &AppHandle) -> AppResult<(Model, String)> {
    let config_file = catalog::load_ai_config_file(app);
    let (model, _resolved, api_key) = resolve_prefs(&config_file)?;
    Ok((model, api_key))
}

/// 配置不可用时的合成错误流(先 start 后 error,终值为错误消息)。
fn error_event_stream(model: &Model, message: &str) -> AssistantMessageEventStream {
    let (stream, writer) = event_stream::<AssistantMessageEvent, AssistantMessage>();
    let error = error_assistant_message(model, message);
    writer.push(AssistantMessageEvent::Start {
        partial: error.clone(),
    });
    writer.push(AssistantMessageEvent::Error {
        reason: StopReason::Error,
        error: error.clone(),
    });
    writer.end(error);
    stream
}

fn error_assistant_message(model: &Model, message: &str) -> AssistantMessage {
    AssistantMessage {
        role: "assistant".to_string(),
        content: Vec::new(),
        api: API_OPENAI_COMPLETIONS.to_string(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        usage: Usage::zero(),
        stop_reason: StopReason::Error,
        error_message: Some(message.to_string()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ts_nanos() / 1_000_000,
    }
}

// ── 事件监听与用量聚合 ───────────────────────────────────────────────

/// AgentEvent → ChatEvent 映射监听器:
/// - TextDelta → TextDelta;ThinkingDelta → ThinkingDelta(思考过程展示用)
/// - tool_execution_start/end → ToolCall / ToolResult
/// - MessageEnd(assistant)→ 累计 usage 并记录上下文占用
/// - 成功 TurnEnd → TurnEnd(附上下文构成估算;错误 attempt 由重试编排层处理,不向前端固化)
fn chat_event_listener(
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

fn tool_result_text(result: &AgentToolResult) -> String {
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

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    format!("{cut}…")
}

fn usage_to_summary(usage: &Usage, context_tokens: i64) -> ChatUsageSummary {
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
fn record_chat_usage(db: &Db, app: &AppHandle, usage: &Usage, duration_ms: i64) {
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
