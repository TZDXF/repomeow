//! `@earendil-works/pi-ai` 的统一类型契约、事件流与 provider adapters。
//!
//! 对齐蓝本:`D:\code\pi\packages\ai`。Rust 以静态 match 替代 TS provider registry;
//! adapter 的请求转换、流事件与终态语义保持一致。

pub mod anthropic_messages;
pub mod event_stream;
pub mod google_generative_ai;
pub mod openai_completions;
pub mod openai_responses;
pub mod overflow;
pub mod retry;
pub mod types;
pub mod validate;

pub use types::*;

use tokio_util::sync::CancellationToken;

use crate::time_util::now_ts_nanos;
use event_stream::event_stream;

/// 按 `Model.api` 分派到与 pi 对齐的 `streamSimple` adapter。
pub fn stream_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
    signal: Option<CancellationToken>,
) -> AssistantMessageEventStream {
    match model.api.as_str() {
        API_OPENAI_COMPLETIONS => {
            openai_completions::stream_openai_completions(model, context, options, signal)
        }
        API_OPENAI_RESPONSES => {
            openai_responses::stream_openai_responses(model, context, options, signal)
        }
        API_ANTHROPIC_MESSAGES => {
            anthropic_messages::stream_anthropic_messages(model, context, options, signal)
        }
        API_GOOGLE_GENERATIVE_AI => {
            google_generative_ai::stream_google_generative_ai(model, context, options, signal)
        }
        _ => unsupported_api_stream(model),
    }
}

/// 按 `Model.api` 分派的非流式(标准)一次性生成:同一请求语义(`stream: false`
/// 或对应的非 SSE 端点),响应体一次性读回并解析为最终 [`AssistantMessage`],
/// 终态语义(stopReason / errorMessage / usage)与流式终态消息一致。
/// 翻译、提交信息、日报/周报等无增量展示需求的单发调用走此路径。
pub async fn complete_simple(
    model: Model,
    context: Context,
    options: Option<SimpleStreamOptions>,
    signal: Option<CancellationToken>,
) -> AssistantMessage {
    match model.api.as_str() {
        API_OPENAI_COMPLETIONS => {
            openai_completions::complete_openai_completions(model, context, options, signal).await
        }
        API_OPENAI_RESPONSES => {
            openai_responses::complete_openai_responses(model, context, options, signal).await
        }
        API_ANTHROPIC_MESSAGES => {
            anthropic_messages::complete_anthropic_messages(model, context, options, signal).await
        }
        API_GOOGLE_GENERATIVE_AI => {
            google_generative_ai::complete_google_generative_ai(model, context, options, signal)
                .await
        }
        _ => unsupported_api_message(&model),
    }
}

fn unsupported_api_message(model: &Model) -> AssistantMessage {
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
        error_message: Some(format!("No API provider registered for api: {}", model.api)),
        raw_stop_reason: None,
        end_turn: None,
        timestamp: now_ts_nanos() / 1_000_000,
    }
}

fn unsupported_api_stream(model: Model) -> AssistantMessageEventStream {
    let (stream, writer) = event_stream::<AssistantMessageEvent, AssistantMessage>();
    let error = unsupported_api_message(&model);
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

