//! 项目问答(chat)Tauri 命令层。
//!
//! 前端经 `chat_send` 发送消息,Rust 侧用 pi Agent(按模型 api 分派的 provider
//! 流 + RepoMeow 工具集)跑完整个对话回合,经 `Channel<ChatEvent>` 回推增量事件;会话按
//! 项目路径隔离,跨消息保留上下文。`chat_abort` 取消进行中的回合,
//! `chat_new_session` 丢弃会话上下文。每次 `chat_send` 结束后把聚合的
//! token 用量写入 `ai_usage_log`(task_type = "chat")。
//!
//! 模型/思考强度/工具权限来自 `ai-config.json` 的 `chat` 段(缺省回退
//! defaultModel),每次 `chat_send` 前重读:思考与权限变化经 `Agent` 的
//! 状态热切换方法就地生效(会话历史保留),模型与密钥由 StreamFn 每次
//! LLM 调用时重读。
//!
//! ask 权限(硬确认):工具集与 all 相同,但五个有副作用工具
//! (`update_wiki` / `regenerate_wiki` / `add_custom_command` /
//! `generate_report` / `set_wiki_model`)执行前经 `AgentLoopConfig.before_tool_call`
//! 钩子拦截,
//! 推 `ToolPermissionRequest` 事件并等待 `chat_tool_permission_respond`
//! 决策(允许继续 / 拒绝或 2 分钟超时则 block);这些工具均带 sequential
//! 标记,含它们的批次整体顺序执行,确认一次最多挂起一个。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent::llm::Usage;
use crate::agent::types::{AgentMessage, TypedMessage};
use crate::agent::Agent;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;
use crate::time_util::now_ts_nanos;
mod events;
mod permission;
mod session;
mod stream;
#[cfg(test)]
mod tests;
mod turn;

use events::*;
use permission::*;
use session::*;
use stream::*;
use turn::*;

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

/// 编辑上一条提问:丢弃会话里最后一条用户消息及其后的整个回答回合
/// (前端随后以新文本重新发送)。会话不存在或无用户消息时幂等空操作;
/// 回合进行中拒绝(busy)。
#[tauri::command]
pub async fn chat_truncate_last_turn(project_path: String) -> AppResult<()> {
    let session_key = clean_str(&project_path);
    let session = chat_sessions().lock().unwrap().get(&session_key).cloned();
    let Some(session) = session else {
        return Ok(());
    };
    if session.busy.load(Ordering::SeqCst) {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "chat_busy: agent is already processing",
        ));
    }
    truncate_last_user_turn(&session.agent);
    Ok(())
}

/// 截掉最后一条 user 消息及其后的全部消息(该提问的回答回合)。返回是否
/// 有截断发生。
fn truncate_last_user_turn(agent: &Agent) -> bool {
    let mut messages = agent.messages();
    let Some(index) = messages
        .iter()
        .rposition(|message| matches!(message, AgentMessage::Message(TypedMessage::User(_))))
    else {
        return false;
    };
    messages.truncate(index);
    agent.set_messages(messages);
    true
}

