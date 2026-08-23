use std::collections::HashSet;
use std::fs;
use std::pin::Pin;

use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use async_openai::types::chat::{
    CompletionUsage, CreateChatCompletionResponse, CreateChatCompletionStreamResponse,
};
use async_openai::Client;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::app_data_dir;
use crate::error::{AppError, AppResult, ErrorCode};

type OpenAiClient = Client<OpenAIConfig>;
type CompatibleStream =
    Pin<Box<dyn Stream<Item = Result<CreateChatCompletionStreamResponse, OpenAIError>> + Send>>;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub ai_base_url: String,
    pub ai_api_key: String,
    pub ai_model: String,
}

impl AiConfig {
    pub fn normalized(mut self) -> Self {
        self.ai_base_url = self.ai_base_url.trim().trim_end_matches('/').to_string();
        self.ai_api_key = self.ai_api_key.trim().to_string();
        self.ai_model = self.ai_model.trim().to_string();
        self
    }

    pub fn validate(&self, require_model: bool) -> AppResult<()> {
        if self.ai_base_url.is_empty()
            || self.ai_api_key.is_empty()
            || (require_model && self.ai_model.is_empty())
        {
            return Err(AppError::coded(ErrorCode::AiNotConfigured, ""));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct TokenUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ChatOutput {
    pub text: String,
    pub usage: Option<TokenUsage>,
}

pub fn load_config(app: &AppHandle) -> AiConfig {
    let value = app_data_dir(app)
        .ok()
        .and_then(|dir| fs::read_to_string(dir.join("settings.json")).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_default();
    AiConfig {
        ai_base_url: value
            .get("aiBaseUrl")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ai_api_key: value
            .get("aiApiKey")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ai_model: value
            .get("aiModel")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
    .normalized()
}

fn client(config: &AiConfig, require_model: bool) -> AppResult<OpenAiClient> {
    config.validate(require_model)?;
    let sdk_config = OpenAIConfig::new()
        .with_api_base(config.ai_base_url.clone())
        .with_api_key(config.ai_api_key.clone());
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| AppError::coded(ErrorCode::AiRequestFailed, e.to_string()))?;
    Ok(Client::with_config(sdk_config).with_http_client(http))
}

fn map_sdk_error(error: OpenAIError) -> AppError {
    match error {
        OpenAIError::ApiError(api) => {
            let code = if api.status_code.as_u16() == 429 {
                ErrorCode::AiRateLimited
            } else if api.status_code.as_u16() == 408
                || api.status_code.as_u16() == 409
                || api.status_code.is_server_error()
            {
                ErrorCode::AiServiceUnavailable
            } else {
                ErrorCode::AiResponseError
            };
            map_api_error(code, api.api_error.message)
        }
        OpenAIError::JSONDeserialize(error, _) => {
            AppError::coded(ErrorCode::AiResponseParseFailed, error.to_string())
        }
        other => AppError::coded(ErrorCode::AiRequestFailed, other.to_string()),
    }
}

fn map_api_error(code: ErrorCode, message: String) -> AppError {
    AppError::ai_provider_error(code, message)
}

fn usage_of(usage: CompletionUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: Some(i64::from(usage.prompt_tokens)),
        output_tokens: Some(i64::from(usage.completion_tokens)),
        total_tokens: Some(i64::from(usage.total_tokens)),
        cached_tokens: usage
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens)
            .map(i64::from),
    }
}

/// 按服务商/模型名给出关闭思考模式的兼容扩展字段。
fn thinking_off_params(base_url: &str, model: &str) -> Map<String, Value> {
    let source = format!("{} {}", base_url.to_lowercase(), model.to_lowercase());
    let model_lower = model.to_lowercase();
    let mut params = Map::new();
    if source.contains("qwen") || source.contains("dashscope") || source.contains("aliyuncs") {
        params.insert("enable_thinking".into(), Value::Bool(false));
        if !source.contains("dashscope") && !source.contains("aliyuncs") {
            params.insert(
                "chat_template_kwargs".into(),
                json!({ "enable_thinking": false }),
            );
        }
    } else if source.contains("glm")
        || source.contains("zhipu")
        || source.contains("bigmodel")
        || source.contains("doubao")
        || source.contains("volces")
    {
        params.insert("thinking".into(), json!({ "type": "disabled" }));
    } else if model_lower.starts_with("step-3") || model_lower.starts_with("step-r") {
        params.insert("reasoning_effort".into(), Value::String("low".into()));
    }
    params
}

fn request_body(
    config: &AiConfig,
    system_prompt: Option<&str>,
    user_prompt: &str,
    thinking_enabled: bool,
    stream: bool,
    max_output_tokens: Option<u32>,
) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = system_prompt {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": user_prompt }));
    let mut body = json!({
        "model": config.ai_model,
        "messages": messages,
        "stream": stream,
    });
    if stream {
        body["stream_options"] = json!({ "include_usage": true });
    }
    if let Some(max_tokens) = max_output_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    if !thinking_enabled {
        if let Some(object) = body.as_object_mut() {
            object.extend(thinking_off_params(&config.ai_base_url, &config.ai_model));
        }
    }
    body
}

pub async fn chat(
    config: &AiConfig,
    system_prompt: Option<&str>,
    user_prompt: &str,
    thinking_enabled: bool,
    max_output_tokens: Option<u32>,
    cancel: Option<&CancellationToken>,
) -> AppResult<ChatOutput> {
    let sdk = client(config, true)?;
    let request = request_body(
        config,
        system_prompt,
        user_prompt,
        thinking_enabled,
        false,
        max_output_tokens,
    );
    let chat = sdk.chat();
    let future = chat.create_byot::<_, CreateChatCompletionResponse>(request);
    let response = if let Some(token) = cancel {
        tokio::select! {
            result = future => result.map_err(map_sdk_error)?,
            _ = token.cancelled() => return Err(AppError::coded(ErrorCode::AiRequestFailed, "canceled")),
        }
    } else {
        future.await.map_err(map_sdk_error)?
    };
    let text = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.clone())
        .ok_or_else(|| AppError::coded(ErrorCode::AiEmptyResponse, ""))?;
    Ok(ChatOutput {
        text: strip_thinking(&text),
        usage: response.usage.map(usage_of),
    })
}

pub async fn stream_chat<F>(
    config: &AiConfig,
    system_prompt: &str,
    user_prompt: &str,
    thinking_enabled: bool,
    cancel: &CancellationToken,
    mut on_text: F,
) -> AppResult<ChatOutput>
where
    F: FnMut(&str),
{
    let sdk = client(config, true)?;
    let request = request_body(
        config,
        Some(system_prompt),
        user_prompt,
        thinking_enabled,
        true,
        None,
    );
    let mut stream: CompatibleStream = sdk
        .chat()
        .create_stream_byot(request)
        .await
        .map_err(map_sdk_error)?;
    let mut accumulated = String::new();
    let mut usage = None;
    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            _ = cancel.cancelled() => return Err(AppError::coded(ErrorCode::AiRequestFailed, "canceled")),
        };
        let Some(item) = item else { break };
        let chunk = item.map_err(map_sdk_error)?;
        for choice in chunk.choices {
            if let Some(content) = choice.delta.content {
                accumulated.push_str(&content);
                on_text(&strip_thinking(&accumulated));
            }
        }
        if let Some(value) = chunk.usage {
            usage = Some(usage_of(value));
        }
    }
    let text = strip_thinking(&accumulated);
    if text.is_empty() {
        return Err(AppError::coded(ErrorCode::AiEmptyResponse, ""));
    }
    Ok(ChatOutput { text, usage })
}

pub async fn list_models(config: &AiConfig) -> AppResult<Vec<String>> {
    let sdk = client(config, false)?;
    let response = sdk.models().list().await.map_err(map_sdk_error)?;
    let mut seen = HashSet::new();
    let mut models: Vec<String> = response
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect();
    models.sort();
    Ok(models)
}

/// 只剥离响应起始位置完整闭合的思考块。
pub fn strip_thinking(text: &str) -> String {
    let mut out = text.trim_start();
    loop {
        if !out
            .get(.."<think>".len())
            .is_some_and(|head| head.eq_ignore_ascii_case("<think>"))
        {
            break;
        }
        let lower = out.to_ascii_lowercase();
        let Some(close) = lower.find("</think>") else {
            return String::new();
        };
        out = out[close + "</think>".len()..].trim_start();
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_normalization_preserves_v1_path() {
        let config = AiConfig {
            ai_base_url: " http://localhost:11434/v1/ ".into(),
            ai_api_key: " key ".into(),
            ai_model: " model ".into(),
        }
        .normalized();
        assert_eq!(config.ai_base_url, "http://localhost:11434/v1");
        assert_eq!(config.ai_api_key, "key");
        assert_eq!(config.ai_model, "model");
    }

    #[test]
    fn provider_specific_thinking_fields_match_existing_behavior() {
        let qwen = thinking_off_params("https://dashscope.aliyuncs.com/v1", "qwen-plus");
        assert_eq!(qwen.get("enable_thinking"), Some(&Value::Bool(false)));
        assert!(!qwen.contains_key("chat_template_kwargs"));

        let local = thinking_off_params("http://localhost/v1", "qwen3");
        assert!(local.contains_key("chat_template_kwargs"));

        let glm = thinking_off_params("https://open.bigmodel.cn/v1", "glm-4");
        assert_eq!(glm.get("thinking"), Some(&json!({ "type": "disabled" })));
    }

    #[test]
    fn max_output_token_api_errors_have_a_specific_code() {
        let error = map_api_error(
            ErrorCode::AiResponseError,
            "invalid params, model[MiniMax-M3] does not support max tokens > 524288 (2013)".into(),
        );
        assert!(error.is_code(ErrorCode::AiMaxOutputTokensExceeded));

        let error = map_api_error(
            ErrorCode::AiResponseError,
            "provider temporarily unavailable".into(),
        );
        assert!(error.is_code(ErrorCode::AiResponseError));
    }

    #[test]
    fn rate_limit_and_server_errors_are_retryable() {
        let provider_error = |status_code| {
            map_sdk_error(OpenAIError::ApiError(
                async_openai::error::ApiErrorResponse {
                    status_code,
                    api_error: async_openai::error::ApiError {
                        message: "temporary provider error".into(),
                        r#type: None,
                        param: None,
                        code: None,
                    },
                },
            ))
        };
        let rate_limited = provider_error(reqwest::StatusCode::TOO_MANY_REQUESTS);
        assert!(rate_limited.is_code(ErrorCode::AiRateLimited));
        assert!(rate_limited.is_retryable_ai_error());

        let unavailable = provider_error(reqwest::StatusCode::SERVICE_UNAVAILABLE);
        assert!(unavailable.is_code(ErrorCode::AiServiceUnavailable));
        assert!(unavailable.is_retryable_ai_error());
    }

    #[test]
    fn strip_thinking_only_removes_leading_blocks() {
        assert_eq!(strip_thinking("<think>x</think>result"), "result");
        assert_eq!(
            strip_thinking("<think>a</think> <THINK>b</THINK> result"),
            "result"
        );
        assert_eq!(
            strip_thinking("body <think>keep</think>"),
            "body <think>keep</think>"
        );
        assert_eq!(strip_thinking("<think>unfinished"), "");
    }
}
