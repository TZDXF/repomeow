use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    ContentBlock, PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome, SessionUpdate, ToolCallLocation, ToolKind,
};

use super::{AcpEvent, AcpEventSender};

const READ_FILE_MAX: usize = 256 * 1024;

pub(super) struct PromptSink {
    pub sender: AcpEventSender,
    pub text: String,
    tool_titles: HashMap<String, String>,
}

impl PromptSink {
    pub(super) fn new(sender: AcpEventSender) -> Self {
        Self {
            sender,
            text: String::new(),
            tool_titles: HashMap::new(),
        }
    }
}

pub(super) type SharedSink = Arc<Mutex<Option<PromptSink>>>;

pub(super) fn route_session_update(sink: &SharedSink, update: SessionUpdate) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content {
                push_chunk(sink, &text.text);
            }
        }
        SessionUpdate::ToolCall(call) => {
            let title = if call.title.is_empty() {
                format!("{:?}", call.kind)
            } else {
                call.title.clone()
            };
            remember_tool_title(sink, call.tool_call_id.0.as_ref(), &title);
            push_activity(
                sink,
                tool_activity_text(&title, &call.locations, call.raw_input.as_ref()),
            );
        }
        SessionUpdate::ToolCallUpdate(call) => {
            let has_path_detail = call
                .fields
                .locations
                .as_ref()
                .is_some_and(|locations| !locations.is_empty())
                || call.fields.raw_input.is_some();
            if !has_path_detail && call.fields.title.is_none() {
                return;
            }
            let call_id = call.tool_call_id.0.as_ref();
            let title = call
                .fields
                .title
                .as_deref()
                .map(str::to_string)
                .or_else(|| known_tool_title(sink, call_id))
                .or_else(|| call.fields.kind.as_ref().map(|kind| format!("{kind:?}")))
                .unwrap_or_else(|| "更新".into());
            remember_tool_title(sink, call_id, &title);
            push_activity(
                sink,
                tool_activity_text(
                    &title,
                    call.fields.locations.as_deref().unwrap_or_default(),
                    call.fields.raw_input.as_ref(),
                ),
            );
        }
        _ => {}
    }
}

fn remember_tool_title(sink: &SharedSink, call_id: &str, title: &str) {
    if let Some(prompt) = sink.lock().unwrap().as_mut() {
        prompt
            .tool_titles
            .insert(call_id.to_string(), title.to_string());
    }
}

fn known_tool_title(sink: &SharedSink, call_id: &str) -> Option<String> {
    sink.lock()
        .unwrap()
        .as_ref()?
        .tool_titles
        .get(call_id)
        .cloned()
}

pub(super) fn tool_activity_text(
    title: &str,
    locations: &[ToolCallLocation],
    raw_input: Option<&serde_json::Value>,
) -> String {
    if let Some(input) = raw_input {
        return format!("{title} {}", summarize_tool_input(input));
    }
    if locations.is_empty() {
        return title.to_string();
    }
    let paths = locations
        .iter()
        .map(|location| location.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("{title} {}", truncate_inline(&paths))
}

fn summarize_tool_input(input: &serde_json::Value) -> String {
    const KEYS: &[&str] = &[
        "file_path",
        "path",
        "command",
        "cmd",
        "pattern",
        "query",
        "url",
        "description",
    ];
    if let Some(obj) = input.as_object() {
        let target = obj
            .get("arguments")
            .and_then(|value| value.as_object())
            .unwrap_or(obj);
        let parts: Vec<String> = KEYS
            .iter()
            .filter_map(|key| target.get(*key))
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            })
            .map(|text| truncate_inline(&text))
            .collect();
        if !parts.is_empty() {
            return parts.join(" ");
        }
    }
    truncate_inline(&input.to_string())
}

fn truncate_inline(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = one_line.chars();
    let truncated: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

pub(super) fn read_tool_activity_text(
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> String {
    let mut text = format!("read {}", path.display());
    if let Some(line) = line {
        text += &format!(":{line}");
    }
    if let Some(limit) = limit {
        text += &format!("+{limit}");
    }
    text
}

fn push_chunk(sink: &SharedSink, delta: &str) {
    let (sender, text) = {
        let mut guard = sink.lock().unwrap();
        match guard.as_mut() {
            Some(prompt) => {
                prompt.text.push_str(delta);
                (prompt.sender.clone(), prompt.text.clone())
            }
            None => return,
        }
    };
    sender.send(AcpEvent::Chunk { text });
}

pub(super) fn push_activity(sink: &SharedSink, text: String) {
    if let Some(prompt) = sink.lock().unwrap().as_ref() {
        prompt.sender.send(AcpEvent::Activity { text });
    }
}

pub(super) fn decide_permission(
    req: &RequestPermissionRequest,
) -> (bool, RequestPermissionOutcome) {
    let allow = !matches!(
        req.tool_call.fields.kind,
        Some(
            ToolKind::Edit
                | ToolKind::Delete
                | ToolKind::Move
                | ToolKind::Execute
                | ToolKind::SwitchMode
        )
    );
    let pick = |primary: PermissionOptionKind, fallback: PermissionOptionKind| {
        req.options
            .iter()
            .find(|option| option.kind == primary)
            .or_else(|| req.options.iter().find(|option| option.kind == fallback))
    };
    let option = if allow {
        pick(
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        )
    } else {
        pick(
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        )
    };
    let outcome = option
        .map(|option| {
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                option.option_id.clone(),
            ))
        })
        .unwrap_or(RequestPermissionOutcome::Cancelled);
    (allow, outcome)
}

/// fs/read_text_file:canonicalize 后限制在 root 内，支持行范围与 256KB 上限。
pub(super) fn read_file_within(
    root: &Path,
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> std::io::Result<String> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canon = full.canonicalize()?;
    if !canon.starts_with(&root_canon) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("路径越界: {}", canon.display()),
        ));
    }
    let bytes = std::fs::read(&canon)?;
    if bytes.len() > READ_FILE_MAX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("超过 {} 字节上限", READ_FILE_MAX),
        ));
    }
    let content = String::from_utf8_lossy(&bytes).to_string();
    match (line, limit) {
        (None, None) => Ok(content),
        _ => {
            let start = line.unwrap_or(1).saturating_sub(1) as usize;
            let take = limit.unwrap_or(u32::MAX) as usize;
            Ok(content
                .lines()
                .skip(start)
                .take(take)
                .collect::<Vec<_>>()
                .join("\n"))
        }
    }
}
