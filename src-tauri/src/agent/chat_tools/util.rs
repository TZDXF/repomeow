use std::sync::Arc;
use serde::Serialize;
use serde_json::{Value};
use crate::agent::types::{AgentTool, AgentToolResult, AgentToolUpdateCallback, ToolExecutionError, ToolExecutionMode};
use crate::error::{AppError, ErrorCode};
use crate::time_util::now_ts_nanos;
use super::*;

// ── 组装辅助 ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) fn tool(
    name: &str,
    label: &str,
    description: &str,
    parameters: Value,
    sequential: bool,
    execute: impl Fn(Value, Option<AgentToolUpdateCallback>) -> ToolFuture + Send + Sync + 'static,
) -> AgentTool {
    AgentTool {
        name: name.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        parameters,
        execution_mode: sequential.then_some(ToolExecutionMode::Sequential),
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, args, _signal, on_update| execute(args, on_update)),
    }
}

pub(super) fn tool_err(error: AppError) -> ToolExecutionError {
    Box::new(error)
}

pub(super) fn text_result(text: impl Into<String>) -> Result<AgentToolResult, ToolExecutionError> {
    Ok(AgentToolResult::text(text))
}

pub(super) fn invalid_arg(name: &str) -> AppError {
    AppError::coded(
        ErrorCode::AiRequestFailed,
        format!("invalid or missing argument: {name}"),
    )
}

pub(super) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(super) fn require_str(args: &Value, key: &str) -> Result<String, ToolExecutionError> {
    arg_str(args, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| tool_err(invalid_arg(key)))
}

pub(super) fn arg_u64_opt(args: &Value, key: &str) -> Result<Option<u64>, ToolExecutionError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| tool_err(invalid_arg(key))),
    }
}

/// 实体参数:含 "::" 视为 entityId 精确匹配,否则视为实体名。
pub(super) fn split_entity_token(value: &str) -> (Option<String>, Option<String>) {
    if value.contains("::") {
        (Some(value.to_string()), None)
    } else {
        (None, Some(value.to_string()))
    }
}

/// 按 UTF-8 边界截断并标注原始长度。
pub(super) fn truncate_bytes(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n…(已截断,原文 {} 字节)", &text[..end], text.len())
}

pub(super) fn pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

/// Rust 侧自生成的关联标识(8-4-4-4-12 的 UUID v4 形状;熵来自 RandomState,
/// 非密码学随机,仅用于 sem/报告请求的 requestId 关联与取消)。
pub(super) fn pseudo_request_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u64(now_ts_nanos() as u64);
    first.write_u64(u64::from(std::process::id()));
    let hi = first.finish();
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u64(hi);
    second.write_u64(now_ts_nanos() as u64);
    let lo = second.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:01x}{:03x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0x0fff,
        8 | ((lo >> 62) & 0x3),
        (lo >> 48) & 0x0fff,
        lo & 0xffff_ffff_ffff,
    )
}


