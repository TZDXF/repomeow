//! Shared request budget for one-shot generation. Unknown models use a conservative window.
use crate::agent::llm::Model;
use crate::commands::usage::estimate_text_tokens;
use crate::error::{AppError, AppResult, ErrorCode};

pub struct RequestBudget {
    model: String,
    pub input: i64,
    pub output: u32,
}
impl RequestBudget {
    pub fn new(model: &Model, system: &str, tools: &str, output: Option<u32>) -> AppResult<Self> {
        let window = if model.context_window > 0 {
            model.context_window
        } else {
            32_768
        };
        let limit = if model.max_tokens > 0 {
            model.max_tokens
        } else {
            4096
        };
        let output = i64::from(output.unwrap_or(4096))
            .min(limit)
            .min(window / 4)
            .max(1) as u32;
        let input = window
            - i64::from(output)
            - 4096
            - estimate_text_tokens(&model.id, system)
            - estimate_text_tokens(&model.id, tools)
            - 32;
        if input <= 0 {
            return Err(AppError::coded(
                ErrorCode::AiResponseError,
                "系统提示词和工具定义已耗尽模型上下文预算",
            ));
        }
        Ok(Self {
            model: model.id.clone(),
            input,
            output,
        })
    }
    pub fn tokens(&self, text: &str) -> i64 {
        estimate_text_tokens(&self.model, text)
    }
    pub fn fits(&self, text: &str) -> bool {
        self.tokens(text) <= self.input
    }
    /// Split losslessly on UTF-8 boundaries; prefer line boundaries for Markdown and logs.
    pub fn split<'a>(&self, text: &'a str, cap: i64) -> AppResult<Vec<&'a str>> {
        if cap <= 0 {
            return Err(AppError::coded(
                ErrorCode::AiResponseError,
                "输入 token 预算不足",
            ));
        }
        let mut rest = text;
        let mut chunks = Vec::new();
        while !rest.is_empty() {
            if self.tokens(rest) <= cap {
                chunks.push(rest);
                break;
            }
            let boundaries: Vec<usize> = rest
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(rest.len()))
                .collect();
            let (mut lo, mut hi) = (0, boundaries.len() - 1);
            while lo < hi {
                let mid = (lo + hi + 1) / 2;
                if self.tokens(&rest[..boundaries[mid]]) <= cap {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let mut end = boundaries[lo];
            if end == 0 {
                return Err(AppError::coded(
                    ErrorCode::AiResponseError,
                    "单个字符超过输入预算",
                ));
            }
            if let Some(line) = rest[..end].rfind('\n') {
                if line > end / 2 && self.tokens(&rest[..line + 1]) <= cap {
                    end = line + 1;
                }
            }
            chunks.push(&rest[..end]);
            rest = &rest[end..];
        }
        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_accounts_for_fixed_costs_and_output() {
        let model = Model::from_settings("test", "http://localhost");
        let a = RequestBudget::new(&model, "", "", None).unwrap();
        let b = RequestBudget::new(&model, "system instructions", "tool schema", None).unwrap();
        assert!(b.input < a.input);
        assert_eq!(a.output, 4096);
    }
    #[test]
    fn rejects_fixed_prompt_overflow_and_honors_model_limits() {
        let mut model = Model::from_settings("test", "http://localhost");
        model.context_window = 8192;
        model.max_tokens = 512;
        let budget = RequestBudget::new(&model, "", "", Some(4096)).unwrap();
        assert_eq!(budget.output, 512);
        assert_eq!(budget.input, 8192 - 512 - 4096 - 32);
        assert!(RequestBudget::new(&model, &"word ".repeat(10000), "", None).is_err());
        model.context_window = 1024;
        assert!(RequestBudget::new(&model, "", "", None).is_err());
    }
    #[test]
    fn splitting_is_lossless_and_bounded() {
        let budget = RequestBudget::new(
            &Model::from_settings("test", "http://localhost"),
            "",
            "",
            None,
        )
        .unwrap();
        let text = "中文🙂 code\n".repeat(200);
        let chunks = budget.split(&text, 32).unwrap();
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|chunk| budget.tokens(chunk) <= 32));
        assert!(budget.split(&text, 0).is_err());
    }
}
