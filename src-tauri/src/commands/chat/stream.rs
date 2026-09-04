use std::sync::{Arc, Mutex};
use tauri::{AppHandle};
use crate::agent::llm::event_stream::event_stream;
use crate::agent::llm::{stream_simple, AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Context, Model, SimpleStreamOptions, StopReason, Usage};
use crate::agent::types::{StreamFn};
use crate::ai::catalog::{self};
use crate::commands::usage::{estimate_text_tokens};
use crate::error::{AppResult};
use crate::time_util::{now_ts_nanos};
use super::*;

/// StreamFn 包装:每次 LLM 调用时重读 AI 配置(模型/密钥可热更新),
/// 并在发起请求前把上下文构成估算写入会话槽;配置缺失时按流契约把失败
/// 编码进事件流,绝不 panic。
pub(super) fn chat_stream_fn(
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
                    stream_simple(model, context, Some(options), signal)
                }
                Err(error) => error_event_stream(&fallback_model, &error.to_string()),
            }
        })
    })
}

/// 上下文构成估算:system prompt 按原文、工具定义与消息按 JSON 序列化文本
/// 计量(复用 ACP 用量兜底的 tiktoken 口径:已知模型选对应编码器,其余
/// 回退 o200k_base)。是占比展示用的近似值,不用于计费。
pub(super) fn estimate_context_breakdown(model_id: &str, context: &Context) -> ChatContextBreakdown {
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
pub(super) fn load_stream_model(app: &AppHandle) -> AppResult<(Model, String)> {
    let config_file = catalog::load_ai_config_file(app);
    let (model, _resolved, api_key) = resolve_prefs(&config_file)?;
    Ok((model, api_key))
}

/// 配置不可用时的合成错误流(先 start 后 error,终值为错误消息)。
pub(super) fn error_event_stream(model: &Model, message: &str) -> AssistantMessageEventStream {
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

pub(super) fn error_assistant_message(model: &Model, message: &str) -> AssistantMessage {
    AssistantMessage {
        role: "assistant".to_string(),
        content: Vec::new(),
        api: model.api.clone(),
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

