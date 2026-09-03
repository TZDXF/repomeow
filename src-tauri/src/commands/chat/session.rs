use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle};
use tokio::sync::oneshot;
use crate::agent::chat_tools::{chat_tools, ChatToolContext};
use crate::agent::llm::{Model, SimpleStreamOptions, Usage};
use crate::agent::types::{AgentLoopConfig, AgentMessage, AgentState, ConvertToLlmFn, Message, ToolExecutionMode, TypedMessage};
use crate::agent::Agent;
use crate::ai::catalog::{self, ChatPermission, ModelRef};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;
use super::*;

/// 已解析的 chat 偏好快照(会话内缓存,变化才热切换)。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ResolvedPrefs {
    pub(super) model_ref: ModelRef,
    pub(super) thinking: String,
    pub(super) permission: ChatPermission,
}

/// 解析当前 chat 偏好 → (模型元数据, 快照, 厂商 api_key)。
/// 未配置/引用失效时返回 AiNotConfigured。
pub(super) fn resolve_prefs(config_file: &catalog::AiConfigFile) -> AppResult<(Model, ResolvedPrefs, String)> {
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
pub(super) fn apply_prefs(app: &AppHandle, session: &ChatSession) -> AppResult<()> {
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

pub(super) fn build_session(
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
        // ask 权限下拦截五个有副作用工具的硬确认门禁(通用 agent core 不动)。
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
pub(super) fn build_system_prompt(project_name: &str, project_path: &str) -> String {
    include_str!("../../ai/prompts/chat-system.md")
        .replace("{{PROJECT_NAME}}", project_name)
        .replace("{{PROJECT_PATH}}", project_path)
}

/// projects 表按 path 查主键(未登记返回 None;路径按 clean_str 归一化)。
pub(super) fn lookup_project_id(db: &Db, project_path: &str) -> Option<i64> {
    let conn = db.0.lock().ok()?;
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        [clean_str(project_path)],
        |row| row.get::<_, i64>(0),
    )
    .ok()
}

/// 对齐 agent.ts defaultConvertToLlm:已知 role 原样转换,Custom 全滤。
pub(super) fn default_convert_to_llm() -> ConvertToLlmFn {
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

