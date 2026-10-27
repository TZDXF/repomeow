//! OpenAI 适配器共享的重试判定和延迟计算。

use crate::time_util::now_ts_nanos;
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// x-should-retry 优先；否则传输错误及 408/409/429/5xx 可重试。
pub(super) fn is_retryable_provider_error(
    status: Option<u16>,
    x_should_retry: Option<&str>,
) -> bool {
    match x_should_retry {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    match status {
        None => true,
        Some(status) => status == 408 || status == 409 || status == 429 || status >= 500,
    }
}

fn parse_retry_after_ms(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn parse_retry_after(value: &str, now_ms: i64) -> Option<f64> {
    let trimmed = value.trim();
    if let Ok(seconds) = trimmed.parse::<f64>() {
        return seconds.is_finite().then_some(seconds * 1000.0);
    }
    let target = chrono::DateTime::parse_from_rfc2822(trimmed).ok()?;
    Some((target.timestamp_millis() - now_ms) as f64)
}

pub(super) fn exponential_retry_delay_ms(retry_index: u32, jitter: f64) -> u64 {
    let base = (0.5 * 2f64.powi(retry_index.min(16) as i32)).min(8.0) * 1000.0;
    let jitter = jitter.clamp(0.0, 1.0);
    (base * (1.0 - jitter * 0.25)) as u64
}

/// 优先使用服务端延迟，超过限制时返回错误；否则使用指数退避。
pub(super) fn next_retry_delay_ms(
    retry_after_ms: Option<&str>,
    retry_after: Option<&str>,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
    now_ms: i64,
    jitter: f64,
    provider_error_message: &str,
) -> Result<u64, String> {
    let validate = |delay_ms: f64| -> Result<u64, String> {
        let max_delay = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS) as f64;
        if max_delay > 0.0 && delay_ms > max_delay {
            return Err(format!(
                "Server requested {}s retry delay (max: {}s). {}",
                (delay_ms / 1000.0).ceil(),
                (max_delay / 1000.0).ceil(),
                provider_error_message
            ));
        }
        Ok(delay_ms.max(0.0) as u64)
    };
    if let Some(delay_ms) = retry_after_ms.and_then(parse_retry_after_ms) {
        return validate(delay_ms);
    }
    if let Some(delay_ms) = retry_after.and_then(|value| parse_retry_after(value, now_ms)) {
        return validate(delay_ms);
    }
    Ok(exponential_retry_delay_ms(retry_index, jitter))
}

/// 一次请求失败及重试所需的响应提示。
pub(super) struct RequestFailure {
    pub(super) message: String,
    pub(super) retryable: bool,
    pub(super) retry_after_ms: Option<String>,
    pub(super) retry_after: Option<String>,
}

/// 仅统一 OpenAI 两种接口的重试循环；构造及发送请求由各适配器负责。
pub(super) async fn send_with_retry<F, Fut>(
    mut send: F,
    max_retries: u32,
    max_retry_delay_ms: Option<u64>,
    signal: Option<&CancellationToken>,
) -> Result<reqwest::Response, (String, bool)>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, RequestFailure>>,
{
    let mut retries_remaining = max_retries;
    let mut retry_index: u32 = 0;
    loop {
        match send().await {
            Ok(response) => return Ok(response),
            Err(failure) => {
                if signal.is_some_and(|token| token.is_cancelled()) {
                    return Err(("Request was aborted".to_string(), true));
                }
                if retries_remaining == 0 || !failure.retryable {
                    return Err((failure.message, false));
                }
                retries_remaining -= 1;
                let now_ms = now_ts_nanos() / 1_000_000;
                let jitter = (now_ts_nanos() % 1_000_000) as f64 / 1_000_000.0;
                let delay_ms = next_retry_delay_ms(
                    failure.retry_after_ms.as_deref(),
                    failure.retry_after.as_deref(),
                    retry_index,
                    max_retry_delay_ms,
                    now_ms,
                    jitter,
                    &failure.message,
                )
                .map_err(|message| (message, false))?;
                retry_index += 1;
                let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
                match signal {
                    None => sleep.await,
                    Some(token) => tokio::select! {
                        _ = sleep => {},
                        _ = token.cancelled() => return Err(("Request was aborted".to_string(), true)),
                    },
                }
            }
        }
    }
}
