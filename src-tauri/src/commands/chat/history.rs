//! 对话历史:SQLite 同时保存前端时间线和可续聊的 Agent 消息(包括工具结果/压缩摘要)。
use rusqlite::{params, Connection, OptionalExtension};

use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistoryEntry {
    id: String,
    title: String,
    updated_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistorySnapshot {
    entry: ChatHistoryEntry,
    state: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistory {
    entries: Vec<ChatHistoryEntry>,
    active: Option<ChatHistorySnapshot>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedContext {
    #[serde(deserialize_with = "deserialize_messages")]
    messages: Vec<AgentMessage>,
    context_tokens: i64,
    breakdown: Option<ChatContextBreakdown>,
    last_compaction_ts: i64,
}

// TypedMessage 的内部 role tag 与载荷自身的 role 字段重叠,不能直接用
// AgentMessage 的 untagged 反序列化(会退化为 Custom,被 LLM 转换器过滤掉)。
// 按 role 直接还原具体载荷,工具结果与 provider reasoning 元数据均原样保留。
fn deserialize_messages<'de, D>(deserializer: D) -> Result<Vec<AgentMessage>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Vec::<Value>::deserialize(deserializer)?
        .into_iter()
        .map(|value| {
            let message = match value.get("role").and_then(Value::as_str) {
                Some("user") => serde_json::from_value(value).map(TypedMessage::User),
                Some("assistant") => serde_json::from_value(value).map(TypedMessage::Assistant),
                Some("toolResult") => serde_json::from_value(value).map(TypedMessage::ToolResult),
                Some("system") => serde_json::from_value(value).map(TypedMessage::System),
                _ => {
                    return value
                        .as_object()
                        .cloned()
                        .map(AgentMessage::Custom)
                        .ok_or_else(|| serde::de::Error::custom("invalid chat history message"))
                }
            }
            .map_err(serde::de::Error::custom)?;
            Ok(AgentMessage::Message(message))
        })
        .collect()
}

fn json_error(error: serde_json::Error) -> AppError {
    AppError::coded(ErrorCode::IoError, error.to_string())
}

fn ensure_idle(path: &str) -> AppResult<Option<ChatSession>> {
    let session = chat_sessions().lock().unwrap().get(path).cloned();
    if session
        .as_ref()
        .is_some_and(|s| s.busy.load(Ordering::SeqCst))
    {
        return Err(AppError::coded(ErrorCode::AiRequestFailed, "chat_busy"));
    }
    Ok(session)
}

fn read_snapshot(conn: &Connection, path: &str, id: &str) -> AppResult<ChatHistorySnapshot> {
    let (title, updated_at, state): (String, i64, String) = conn.query_row(
        "SELECT title, updated_at, state FROM chat_history WHERE project_path = ?1 AND id = ?2",
        params![path, id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(ChatHistorySnapshot {
        entry: ChatHistoryEntry {
            id: id.into(),
            title,
            updated_at,
        },
        state: serde_json::from_str(&state).map_err(json_error)?,
    })
}

fn set_active(conn: &Connection, path: &str, id: Option<&str>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO chat_active_sessions(project_path, session_id) VALUES (?1, ?2)
         ON CONFLICT(project_path) DO UPDATE SET session_id = excluded.session_id",
        params![path, id],
    )?;
    Ok(())
}

pub(super) fn clear_active(db: &Db, path: &str) -> AppResult<()> {
    set_active(&db.0.lock().unwrap(), &clean_str(path), None)
}

/// 恢复时不依赖 AI 配置;真正发送时才构建 Agent 并装入所选历史上下文。
pub(super) fn restore_active_context(db: &Db, path: &str, session: &ChatSession) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    let raw: Option<String> = conn
        .query_row(
            "SELECT h.context FROM chat_history h JOIN chat_active_sessions a
         ON a.project_path = h.project_path AND a.session_id = h.id WHERE h.project_path = ?1",
            [path],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(raw) = raw {
        let context: SavedContext = serde_json::from_str(&raw).map_err(json_error)?;
        session.agent.set_messages(context.messages);
        *session.context_tokens.lock().unwrap() = context.context_tokens;
        *session.breakdown.lock().unwrap() = context.breakdown;
        *session.last_compaction_ts.lock().unwrap() = context.last_compaction_ts;
    }
    Ok(())
}

#[tauri::command]
pub fn chat_history_get(db: State<'_, Db>, project_path: String) -> AppResult<ChatHistory> {
    get_history(&db, &project_path)
}

fn get_history(db: &Db, project_path: &str) -> AppResult<ChatHistory> {
    let path = clean_str(project_path);
    let conn = db.0.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, title, updated_at FROM chat_history WHERE project_path = ?1 ORDER BY updated_at DESC, id DESC",
    )?;
    let entries = stmt
        .query_map([&path], |row| {
            Ok(ChatHistoryEntry {
                id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let id: Option<String> = conn
        .query_row(
            "SELECT session_id FROM chat_active_sessions WHERE project_path = ?1",
            [&path],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let active = id.map(|id| read_snapshot(&conn, &path, &id)).transpose()?;
    Ok(ChatHistory { entries, active })
}

#[tauri::command]
pub fn chat_history_save(
    db: State<'_, Db>,
    project_path: String,
    session_id: String,
    title: String,
    state: Value,
) -> AppResult<ChatHistoryEntry> {
    save_history(&db, &project_path, session_id, title, state)
}

fn save_history(
    db: &Db,
    project_path: &str,
    session_id: String,
    title: String,
    state: Value,
) -> AppResult<ChatHistoryEntry> {
    let path = clean_str(project_path);
    let session = ensure_idle(&path)?;
    let context = session.map(|s| SavedContext {
        messages: s.agent.messages(),
        context_tokens: *s.context_tokens.lock().unwrap(),
        breakdown: s.breakdown.lock().unwrap().clone(),
        last_compaction_ts: *s.last_compaction_ts.lock().unwrap(),
    });
    let context = context
        .map(|c| serde_json::to_string(&c))
        .transpose()
        .map_err(json_error)?;
    let state = serde_json::to_string(&state).map_err(json_error)?;
    let entry = ChatHistoryEntry {
        id: session_id,
        title,
        updated_at: crate::time_util::now_ts(),
    };
    let mut conn = db.0.lock().unwrap();
    let tx = conn.transaction()?;
    // Agent 尚未重建时保留原上下文,避免查看历史后直接切换/新建导致上下文被抹掉。
    tx.execute(
        "INSERT INTO chat_history(project_path, id, title, updated_at, state, context)
         VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, ?7))
         ON CONFLICT(project_path, id) DO UPDATE SET title = excluded.title,
         updated_at = excluded.updated_at, state = excluded.state,
         context = COALESCE(?6, chat_history.context)",
        params![
            path,
            entry.id,
            entry.title,
            entry.updated_at,
            state,
            context,
            serde_json::to_string(&SavedContext::default()).map_err(json_error)?
        ],
    )?;
    set_active(&tx, &path, Some(&entry.id))?;
    tx.commit()?;
    Ok(entry)
}

#[tauri::command]
pub fn chat_history_select(
    db: State<'_, Db>,
    project_path: String,
    session_id: String,
) -> AppResult<ChatHistorySnapshot> {
    select_history(&db, &project_path, &session_id)
}

fn select_history(db: &Db, project_path: &str, session_id: &str) -> AppResult<ChatHistorySnapshot> {
    let path = clean_str(project_path);
    ensure_idle(&path)?;
    let conn = db.0.lock().unwrap();
    let snapshot = read_snapshot(&conn, &path, session_id)?;
    // 先确认历史存在且可反序列化,再切换,失败时原会话保持不变。
    set_active(&conn, &path, Some(session_id))?;
    chat_sessions().lock().unwrap().remove(&path);
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn database() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init(&conn).unwrap();
        Db(Mutex::new(conn))
    }

    #[test]
    fn history_survives_new_session_and_is_project_scoped() {
        let db = database();
        let path = "C:/chat-history-tests/project";
        let state = json!({ "messages": [{"role": "user", "content": "hello"}] });
        save_history(&db, path, "one".into(), "hello".into(), state.clone()).unwrap();
        let history = get_history(&db, path).unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.active.unwrap().state, state);
        assert!(get_history(&db, "C:/chat-history-tests/other")
            .unwrap()
            .entries
            .is_empty());
        clear_active(&db, path).unwrap();
        let history = get_history(&db, path).unwrap();
        assert!(history.active.is_none());
        assert_eq!(history.entries.len(), 1);
        assert_eq!(select_history(&db, path, "one").unwrap().state, state);
        assert_eq!(
            get_history(&db, path).unwrap().active.unwrap().entry.id,
            "one"
        );
        assert!(select_history(&db, path, "missing").is_err());
        assert_eq!(
            get_history(&db, path).unwrap().active.unwrap().entry.id,
            "one"
        );
    }

    #[test]
    fn saving_loaded_history_without_agent_preserves_tool_and_compaction_context() {
        let db = database();
        let path = "C:/chat-history-tests/context";
        save_history(&db, path, "one".into(), "first".into(), json!({})).unwrap();
        let tool_result = serde_json::from_value(json!({
            "role": "toolResult", "toolCallId": "read-1", "toolName": "read_file",
            "content": [{"type": "text", "text": "file contents"}], "isError": false, "timestamp": 1,
        })).unwrap();
        let assistant = serde_json::from_value(json!({
            "role": "assistant", "api": "openai-completions", "provider": "test", "model": "test",
            "content": [{"type": "thinking", "thinking": "reasoning", "thinkingSignature": "opaque-signature"}],
            "usage": {}, "stopReason": "stop", "timestamp": 1,
        })).unwrap();
        let summary = serde_json::from_value(json!({
            "role": "compactionSummary", "summary": "keep this summary", "timestamp": 2,
        }))
        .unwrap();
        let context = SavedContext {
            messages: vec![
                AgentMessage::user_text("original", 1),
                AgentMessage::Message(TypedMessage::Assistant(assistant)),
                AgentMessage::Message(TypedMessage::ToolResult(tool_result)),
                AgentMessage::Custom(summary),
            ],
            context_tokens: 123,
            last_compaction_ts: 2,
            breakdown: Some(ChatContextBreakdown {
                system_prompt: 1,
                tools: 2,
                messages: 120,
            }),
        };
        let raw = serde_json::to_string(&context).unwrap();
        db.0.lock()
            .unwrap()
            .execute(
                "UPDATE chat_history SET context = ?1 WHERE project_path = ?2",
                params![raw, clean_str(path)],
            )
            .unwrap();
        save_history(
            &db,
            path,
            "one".into(),
            "first".into(),
            json!({"messages": []}),
        )
        .unwrap();
        let saved: String =
            db.0.lock()
                .unwrap()
                .query_row(
                    "SELECT context FROM chat_history WHERE project_path = ?1",
                    [clean_str(path)],
                    |r| r.get(0),
                )
                .unwrap();
        let restored: SavedContext = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.messages, context.messages);
        assert_eq!(restored.context_tokens, 123);
        assert_eq!(restored.last_compaction_ts, 2);
        assert_eq!(restored.breakdown.unwrap().tools, 2);
        assert_eq!(get_history(&db, path).unwrap().entries.len(), 1);
    }
}
