//! 上下文溢出检测:对齐 `packages/ai/src/utils/overflow.ts`。
//!
//! 覆盖三类信号:
//! 1. 错误文本模式:多数 provider 以 stop_reason=error + 特定错误文案返回;
//! 2. 静默溢出(z.ai):请求被接受但 usage.input + cache_read 超过上下文窗口;
//! 3. 截断溢出(小米 MiMo):输入被截到恰好填满窗口,stop_reason=length 且
//!    output=0(没有余量生成)。
//!
//! 自定义 provider 的错误文案若不在模式表内,需要在对应 adapter 归一化,
//! 或在此追加模式。

use std::sync::OnceLock;

use regex::Regex;

use super::types::{AssistantMessage, StopReason};

/// 溢出错误文案模式(逐条对齐 TS `OVERFLOW_PATTERNS`,注释标注厂商)。
fn overflow_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)prompt is too long",                                  // Anthropic token 溢出
            r"(?i)request_too_large", // Anthropic 请求字节溢出(HTTP 413)
            r"(?i)input is too long for requested model", // Amazon Bedrock
            r"(?i)exceeds the context window", // OpenAI(Completions & Responses API)
            r"(?i)exceeds (?:the )?(?:model'?s )?maximum context length(?: of [\d,]+ tokens?|\s*\([\d,]+\))", // OpenAI 兼容代理(LiteLLM)
            r"(?i)input token count.*exceeds the maximum",              // Google (Gemini)
            r"(?i)maximum prompt length is \d+",                        // xAI (Grok)
            r"(?i)reduce the length of the messages",                   // Groq
            r"(?i)maximum context length is \d+ tokens", // OpenRouter(多数后端)
            r"(?i)exceeds (?:the )?maximum allowed input length of [\d,]+ tokens?", // OpenRouter/Poolside
            r"(?i)input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)", // Together AI
            r"(?i)exceeds the limit of \d+",                           // GitHub Copilot
            r"(?i)exceeds the available context size",                 // llama.cpp server
            r"(?i)greater than the context length",                    // LM Studio
            r"(?i)context window exceeds limit",                       // MiniMax
            r"(?i)exceeded model token limit",                         // Kimi For Coding
            r"(?i)too large for model with \d+ maximum context length", // Mistral
            r"(?i)prompt has [\d,]+ tokens?, but the configured context size is [\d,]+ tokens?", // DS4 server
            r"(?i)model_context_window_exceeded", // z.ai 非标准 finish_reason 文本
            r"(?i)prompt too long; exceeded (?:max )?context length", // Ollama 显式溢出
            r"(?i)range of input length should be",                   // DashScope / Qwen
            r"(?i)context[_ ]length[_ ]exceeded",                     // 通用兜底
            r"(?i)too many tokens",                                   // 通用兜底
            r"(?i)token limit exceeded",                              // 通用兜底
            r"(?i)^4(?:00|13)\s*(?:status code)?\s*\(no body\)",      // Cerebras: 400/413 无 body
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("overflow pattern must compile"))
        .collect()
    })
}

/// 非溢出错误模式(限流/服务不可用等),即使同时命中溢出模式也排除
/// (对齐 TS `NON_OVERFLOW_PATTERNS`;如 Bedrock 的 "Too many tokens, please wait")。
fn non_overflow_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"(?i)^(Throttling error|Service unavailable):", // AWS Bedrock 人类可读前缀
            r"(?i)rate limit",                               // 通用限流
            r"(?i)too many requests",                        // 通用 HTTP 429
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("non-overflow pattern must compile"))
        .collect()
    })
}

/// 该 assistant 消息是否表示上下文溢出(对齐 TS `isContextOverflow`)。
/// `context_window <= 0` 时只做错误文本匹配(静默/截断信号需要窗口大小)。
pub fn is_context_overflow(message: &AssistantMessage, context_window: i64) -> bool {
    // 1. 错误文本模式。
    if message.stop_reason == StopReason::Error {
        if let Some(error) = message.error_message.as_deref() {
            let excluded = non_overflow_patterns().iter().any(|p| p.is_match(error));
            if !excluded && overflow_patterns().iter().any(|p| p.is_match(error)) {
                return true;
            }
        }
    }

    // 2. 静默溢出(z.ai):成功返回但输入已超过窗口。
    if context_window > 0 && message.stop_reason == StopReason::Stop {
        let input_tokens = message.usage.input + message.usage.cache_read;
        if input_tokens > context_window {
            return true;
        }
    }

    // 3. 截断溢出(MiMo):length + output=0 + 输入填满窗口 ≥99%。
    if context_window > 0 && message.stop_reason == StopReason::Length && message.usage.output == 0 {
        let input_tokens = message.usage.input + message.usage.cache_read;
        if (input_tokens as f64) >= context_window as f64 * 0.99 {
            return true;
        }
    }

    false
}

/// length 截断是否低于预期输出上限(对齐 TS `isRecoverableLength`):
/// 可能由上下文压力或 provider 侧截断引起,调用方可以做一次有界的
/// 压缩重试。`desired_max_output` 必须是未经上下文钳制的原始上限。
pub fn is_recoverable_length(message: &AssistantMessage, desired_max_output: i64) -> bool {
    message.stop_reason == StopReason::Length
        && desired_max_output > 0
        && message.usage.output < desired_max_output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::llm::types::Usage;

    fn assistant(stop: StopReason, error: Option<&str>, usage: Usage) -> AssistantMessage {
        AssistantMessage {
            role: "assistant".to_string(),
            content: Vec::new(),
            api: "openai-completions".to_string(),
            provider: "openai".to_string(),
            model: "gpt-test".to_string(),
            response_model: None,
            response_id: None,
            usage,
            stop_reason: stop,
            error_message: error.map(str::to_string),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        }
    }

    #[test]
    fn detects_provider_overflow_error_text() {
        let cases = [
            "prompt is too long: 213462 tokens > 200000 maximum",
            "413 {\"error\":{\"type\":\"request_too_large\"}}",
            "Your input exceeds the context window of this model",
            "Requested token count exceeds the model's maximum context length of 131072 tokens",
            "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)",
            "This model's maximum prompt length is 131072 but the request contains 537812 tokens",
            "Please reduce the length of the messages or completion",
            "This endpoint's maximum context length is 200000 tokens. However, you requested about 210000 tokens",
            "The input (300000 tokens) is longer than the model's context length (262144 tokens).",
            "prompt token count of 210000 exceeds the limit of 200000",
            "the request exceeds the available context size, try increasing it",
            "invalid params, context window exceeds limit",
            "Your request exceeded model token limit: 131072 (requested: 140000)",
            "Prompt has 90000 tokens, but the configured context size is 8192 tokens",
            "prompt too long; exceeded max context length by 12 tokens",
            "Range of input length should be [1, 32768]",
            "400 status code (no body)",
        ];
        for case in cases {
            let message = assistant(StopReason::Error, Some(case), Usage::zero());
            assert!(is_context_overflow(&message, 0), "should detect: {case}");
        }
    }

    #[test]
    fn excludes_non_overflow_errors() {
        let cases = [
            "Throttling error: Too many tokens, please wait before trying again.",
            "Service unavailable: too many tokens in queue",
            "rate limit exceeded, retry later",
            "429 too many requests",
        ];
        for case in cases {
            let message = assistant(StopReason::Error, Some(case), Usage::zero());
            assert!(!is_context_overflow(&message, 0), "should exclude: {case}");
        }
    }

    #[test]
    fn detects_silent_overflow_by_usage() {
        let mut usage = Usage::zero();
        usage.input = 210_000;
        let message = assistant(StopReason::Stop, None, usage);
        assert!(is_context_overflow(&message, 200_000));
        assert!(!is_context_overflow(&message, 300_000));
    }

    #[test]
    fn detects_mimo_truncation() {
        let usage = Usage {
            input: 198_500,
            output: 0,
            cache_read: 1_600,
            ..Usage::zero()
        };
        let message = assistant(StopReason::Length, None, usage);
        assert!(is_context_overflow(&message, 200_000));
        // output > 0 的普通 length 不算截断溢出
        let usage = Usage {
            input: 199_000,
            output: 10,
            ..Usage::zero()
        };
        let message = assistant(StopReason::Length, None, usage);
        assert!(!is_context_overflow(&message, 200_000));
    }

    #[test]
    fn recoverable_length_requires_output_below_limit() {
        let usage = Usage {
            output: 100,
            ..Usage::zero()
        };
        let message = assistant(StopReason::Length, None, usage.clone());
        assert!(is_recoverable_length(&message, 8192));
        assert!(!is_recoverable_length(&message, 100));
        assert!(!is_recoverable_length(&message, 0));
        let stop = assistant(StopReason::Stop, None, usage);
        assert!(!is_recoverable_length(&stop, 8192));
    }
}
