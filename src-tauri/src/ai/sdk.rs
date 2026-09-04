use std::collections::HashSet;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::agent::llm::{
    stream_simple, AssistantContent, Context, InputKind, Message, Model, ModelCost, ModelCostRates,
    SimpleStreamOptions, StopReason, UserContent, UserMessage, API_ANTHROPIC_MESSAGES,
    API_GOOGLE_GENERATIVE_AI, API_OPENAI_COMPLETIONS, API_OPENAI_RESPONSES,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::time_util::now_ts_nanos;

fn default_api() -> String {
    API_OPENAI_COMPLETIONS.to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub ai_base_url: String,
    pub ai_api_key: String,
    pub ai_model: String,
    #[serde(default = "default_api")]
    pub api: String,
    #[serde(skip)]
    pub resolved_model: Option<Model>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            ai_base_url: String::new(),
            ai_api_key: String::new(),
            ai_model: String::new(),
            api: default_api(),
            resolved_model: None,
        }
    }
}

impl AiConfig {
    pub fn normalized(mut self) -> Self {
        self.ai_base_url = self.ai_base_url.trim().trim_end_matches('/').to_string();
        self.ai_api_key = self.ai_api_key.trim().to_string();
        self.ai_model = self.ai_model.trim().to_string();
        self.api = self.api.trim().to_string();
        if self.api.is_empty() {
            self.api = default_api();
        }
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

    fn model(&self) -> Model {
        self.resolved_model.clone().unwrap_or_else(|| Model {
            id: self.ai_model.clone(),
            name: self.ai_model.clone(),
            api: self.api.clone(),
            provider: "custom".to_string(),
            base_url: self.ai_base_url.clone(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![InputKind::Text],
            cost: ModelCost {
                rates: ModelCostRates {
                    input: 0.0,
                    output: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                tiers: None,
            },
            context_window: 0,
            max_tokens: 0,
            sampling_params: None,
            headers: None,
            compat: None,
        })
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
    let file = crate::ai::catalog::load_ai_config_file(app);
    crate::ai::catalog::legacy_ai_config(&file).normalized()
}

pub fn load_config_at(data_dir: &std::path::Path) -> AiConfig {
    let file = crate::ai::catalog::load_ai_config_file_at(data_dir);
    crate::ai::catalog::legacy_ai_config(&file).normalized()
}

fn map_assistant_error(message: &str) -> AppError {
    let lower = message.to_ascii_lowercase();
    let code = if lower.contains("429") || lower.contains("rate limit") {
        ErrorCode::AiRateLimited
    } else if lower.contains("408")
        || lower.contains("409")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("504")
        || lower.contains("service unavailable")
        || lower.contains("overloaded")
    {
        ErrorCode::AiServiceUnavailable
    } else {
        ErrorCode::AiResponseError
    };
    AppError::ai_provider_error(code, message.to_string())
}

pub async fn chat(
    config: &AiConfig,
    system_prompt: Option<&str>,
    user_prompt: &str,
    thinking_enabled: bool,
    max_output_tokens: Option<u32>,
    cancel: Option<&CancellationToken>,
) -> AppResult<ChatOutput> {
    config.validate(true)?;
    let model = config.model();
    let context = Context {
        system_prompt: system_prompt.map(str::to_string),
        messages: vec![Message::User(UserMessage {
            role: "user".to_string(),
            content: UserContent::Text(user_prompt.to_string()),
            timestamp: now_ts_nanos() / 1_000_000,
        })],
        tools: Vec::new(),
    };
    let options = SimpleStreamOptions {
        api_key: Some(config.ai_api_key.clone()),
        max_tokens: max_output_tokens,
        reasoning: thinking_enabled.then_some(crate::agent::llm::ThinkingLevel::Medium),
        ..Default::default()
    };
    let mut stream = stream_simple(model, context, Some(options), cancel.cloned());
    while stream.next().await.is_some() {}
    let assistant = stream.result().await;
    if matches!(assistant.stop_reason, StopReason::Error | StopReason::Aborted) {
        return Err(map_assistant_error(
            assistant.error_message.as_deref().unwrap_or("AI request failed"),
        ));
    }
    let text = assistant
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    if text.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::AiEmptyResponse, ""));
    }
    Ok(ChatOutput {
        text: strip_thinking(&text),
        usage: Some(TokenUsage {
            input_tokens: Some(assistant.usage.input + assistant.usage.cache_read),
            output_tokens: Some(assistant.usage.output),
            total_tokens: Some(assistant.usage.total_tokens),
            cached_tokens: Some(assistant.usage.cache_read),
        }),
    })
}

fn model_list_url(config: &AiConfig) -> AppResult<String> {
    let base = config.ai_base_url.trim_end_matches('/');
    match config.api.as_str() {
        API_OPENAI_COMPLETIONS | API_OPENAI_RESPONSES => Ok(format!("{base}/models")),
        API_ANTHROPIC_MESSAGES => Ok(format!("{base}/v1/models")),
        API_GOOGLE_GENERATIVE_AI => Ok(format!("{base}/models?key={}", config.ai_api_key)),
        api => Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            format!("暂不支持 AI API 类型「{api}」"),
        )),
    }
}

pub async fn list_models(config: &AiConfig) -> AppResult<Vec<String>> {
    config.validate(false)?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| AppError::coded(ErrorCode::AiRequestFailed, error.to_string()))?;
    let mut request = client.get(model_list_url(config)?);
    request = match config.api.as_str() {
        API_ANTHROPIC_MESSAGES => request
            .header("x-api-key", &config.ai_api_key)
            .header("anthropic-version", "2023-06-01"),
        API_GOOGLE_GENERATIVE_AI => request,
        _ => request.bearer_auth(&config.ai_api_key),
    };
    let response = request
        .send()
        .await
        .map_err(|error| AppError::coded(ErrorCode::AiRequestFailed, error.to_string()))?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| AppError::coded(ErrorCode::AiResponseParseFailed, error.to_string()))?;
    if !status.is_success() {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("model list request failed");
        return Err(map_assistant_error(&format!("{}: {message}", status.as_u16())));
    }
    let mut seen = HashSet::new();
    let mut models: Vec<String> = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("id").or_else(|| model.get("name")).and_then(Value::as_str))
        .map(|id| id.strip_prefix("models/").unwrap_or(id).trim().to_string())
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect();
    models.sort();
    Ok(models)
}

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
    fn config_normalization_preserves_api() {
        let config = AiConfig {
            ai_base_url: " http://localhost:11434/v1/ ".into(),
            ai_api_key: " key ".into(),
            ai_model: " model ".into(),
            api: " anthropic-messages ".into(),
            resolved_model: None,
        }
        .normalized();
        assert_eq!(config.ai_base_url, "http://localhost:11434/v1");
        assert_eq!(config.ai_api_key, "key");
        assert_eq!(config.ai_model, "model");
        assert_eq!(config.api, API_ANTHROPIC_MESSAGES);
    }

    #[test]
    fn model_list_urls_follow_wire_api() {
        let mut config = AiConfig {
            ai_base_url: "https://example.com".into(),
            ai_api_key: "key".into(),
            ..Default::default()
        };
        assert_eq!(model_list_url(&config).unwrap(), "https://example.com/models");
        config.api = API_ANTHROPIC_MESSAGES.into();
        assert_eq!(model_list_url(&config).unwrap(), "https://example.com/v1/models");
        config.api = API_GOOGLE_GENERATIVE_AI.into();
        assert_eq!(
            model_list_url(&config).unwrap(),
            "https://example.com/models?key=key"
        );
    }

    #[test]
    fn max_output_token_api_errors_have_a_specific_code() {
        let error = map_assistant_error(
            "invalid params, model[MiniMax-M3] does not support max tokens > 524288 (2013)",
        );
        assert!(error.is_code(ErrorCode::AiMaxOutputTokensExceeded));
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
