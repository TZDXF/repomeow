//! Assistant 调用重试策略:对齐 pi-ai `utils/retry.ts`。
//!
//! provider 内层请求重试与本模块无关。这里仅分类最终 assistant error，
//! 并为会话编排层提供有界指数退避和可取消等待。

use std::sync::OnceLock;

use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::{AssistantMessage, StopReason};

/// pi coding-agent 普通对话的默认自动重试次数。
pub const DEFAULT_MAX_RETRIES: u32 = 10;
/// pi coding-agent 普通对话的默认退避基数。
pub const DEFAULT_BASE_DELAY_MS: u64 = 2_000;
/// agent 层指数退避的单次等待上限;蓝本默认 60 秒。
pub const DEFAULT_MAX_AGENT_RETRY_DELAY_MS: u64 = 60_000;

fn non_retryable_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            "(?i)GoUsageLimitError|FreeUsageLimitError|Monthly usage limit reached|available balance|insufficient_quota|out of budget|quota exceeded|billing|subscription_sharing_usage_limit_exceeded|subscription_sharing_workplace_individual_usage_limit_exceeded",
        )
        .expect("static non-retryable regex")
    })
}

fn retryable_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            "(?i)overloaded|server_busy|servers are currently busy|currently experiencing high demand|model is at capacity|rate.?limit|too many requests|(?:^|[^a-z0-9])(?:429|500|502|503|504|520|524)(?:$|[^a-z0-9])|service.?(?:is.?)?(?:temporarily.?)?unavailable|server.?error|internal.?error|provider.?returned.?(?:an.?)?error|exceeded request buffer limit while retrying upstream|unable to forward upstream|upstream.?request.?failed|upstream_stream_read_error|stream was interrupted|http/?2 stream failed|network.?error|connection.?error|connection.?refused|connection.?lost|other side closed|fetch failed|request to \\S+ failed, reason:|getaddrinfo|ENOTFOUND|EAI_AGAIN|upstream.?connect|reset before headers|socket hang up|socket connection was closed|timed? out|timeout|terminated|websocket.?closed|websocket.?error|ended without|stream ended before message_stop|stream ended before a terminal response event|http2 request did not get a response|stream disconnected before completion|pending stream has been canceled|retry delay|you can retry your request|try your request again|please retry your request|ResourceExhausted|subscription_sharing_usage_unavailable|subscription_sharing_user_unavailable",
        )
        .expect("static retryable regex")
    })
}

/// 判断最终 assistant error 是否为可恢复的 provider/transport 瞬态错误。
pub fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let Some(error_message) = message.error_message.as_deref() else {
        return false;
    };
    if non_retryable_pattern().is_match(error_message) {
        return false;
    }
    retryable_pattern().is_match(error_message)
}

/// 第 `attempt` 次重试（1-based）的等待时间；`max_delay_ms` 可覆盖默认上限。
pub fn retry_delay_ms(base_delay_ms: u64, attempt: u32, max_delay_ms: Option<u64>) -> u64 {
    let exponent = attempt.saturating_sub(1).min(63);
    let backoff = base_delay_ms.saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX));
    backoff.min(max_delay_ms.unwrap_or(DEFAULT_MAX_AGENT_RETRY_DELAY_MS))
}

/// 等待指定退避时间；取消时立即返回 `false`，正常到期返回 `true`。
pub async fn sleep_with_cancel(delay_ms: u64, signal: &CancellationToken) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => true,
        _ = signal.cancelled() => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::{AssistantContent, Usage};

    fn assistant_error(message: Option<&str>, reason: StopReason) -> AssistantMessage {
        AssistantMessage {
            role: "assistant".to_string(),
            content: Vec::<AssistantContent>::new(),
            api: "openai-completions".to_string(),
            provider: "test".to_string(),
            model: "test".to_string(),
            response_model: None,
            response_id: None,
            usage: Usage::zero(),
            stop_reason: reason,
            error_message: message.map(str::to_string),
            raw_stop_reason: None,
            end_turn: None,
            duration_ms: None,
            thinking_level: None,
            timestamp: 0,
        }
    }

    #[test]
    fn classifies_transient_provider_and_transport_errors() {
        for message in [
            "429: rate limited",
            "503 service unavailable",
            "Provider returned error",
            "socket connection was closed",
            "stream ended before a terminal response event",
            "ResourceExhausted",
            "upstream_error: Upstream request failed",
            "upstream_stream_read_error",
            "Unable to forward upstream stream",
            "The service is temporarily unavailable",
            "server_busy",
            "stream disconnected before completion: stream closed before response.completed",
            "HTTP/2 stream failed",
        ] {
            assert!(
                is_retryable_assistant_error(&assistant_error(Some(message), StopReason::Error)),
                "expected retryable: {message}",
            );
        }
    }

    #[test]
    fn does_not_treat_digits_inside_ids_or_byte_counts_as_http_statuses() {
        for message in ["req_011CU503xyz", "5242880 bytes", "request failed: x500y"] {
            assert!(!is_retryable_assistant_error(&assistant_error(
                Some(message),
                StopReason::Error
            )));
        }
        assert!(is_retryable_assistant_error(&assistant_error(
            Some("503: upstream unavailable"),
            StopReason::Error
        )));
    }

    #[test]
    fn rejects_quota_billing_and_non_error_messages() {
        for message in [
            "429 insufficient_quota",
            "quota exceeded",
            "out of budget",
            "billing is disabled",
            "context window exceeded",
        ] {
            assert!(
                !is_retryable_assistant_error(&assistant_error(Some(message), StopReason::Error)),
                "expected terminal: {message}",
            );
        }
        assert!(!is_retryable_assistant_error(&assistant_error(
            Some("429"),
            StopReason::Aborted,
        )));
        assert!(!is_retryable_assistant_error(&assistant_error(
            None,
            StopReason::Error,
        )));
    }

    #[test]
    fn computes_pi_exponential_backoff() {
        assert_eq!(retry_delay_ms(2_000, 1, None), 2_000);
        assert_eq!(retry_delay_ms(2_000, 2, None), 4_000);
        assert_eq!(retry_delay_ms(2_000, 3, None), 8_000);
    }

    #[test]
    fn caps_agent_retry_backoff() {
        assert_eq!(retry_delay_ms(2_000, 7, Some(30_000)), 30_000);
        assert_eq!(
            retry_delay_ms(2_000, 10, None),
            DEFAULT_MAX_AGENT_RETRY_DELAY_MS
        );
        assert_eq!(retry_delay_ms(1, 63, Some(u64::MAX)), 1_u64 << 62);
    }

    #[tokio::test]
    async fn cancel_interrupts_backoff() {
        let signal = CancellationToken::new();
        signal.cancel();
        assert!(!sleep_with_cancel(60_000, &signal).await);
    }
}
