//! JSONL 解码错误与辅助:对齐 `packages/agent/src/harness/session/jsonl/errors.ts`。

use std::sync::Arc;

use thiserror::Error;

use crate::agent::harness::session::types::{SessionError, SessionErrorCode};
use crate::agent::harness::types::{FileError, Result};

/// JSONL 行解码错误(kind 限定 syntax/schema;对齐 TS `JsonlDecodeError`)。
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct JsonlDecodeError {
    pub kind: JsonlDecodeErrorKind,
    pub message: String,
    #[source]
    pub cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

/// 解码错误类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonlDecodeErrorKind {
    Syntax,
    Schema,
}

impl JsonlDecodeError {
    pub fn new(kind: JsonlDecodeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(mut self, cause: Arc<dyn std::error::Error + Send + Sync>) -> Self {
        self.cause = Some(cause);
        self
    }
}

/// 把文件错误转换为 `SessionError`:`not_found` 保持,其余归为 `storage`
/// (对齐 TS `fileResult`)。
pub fn file_result<T>(result: Result<T, FileError>, message: &str) -> Result<T, SessionError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let code = if error.code == crate::agent::harness::types::FileErrorCode::NotFound {
                SessionErrorCode::NotFound
            } else {
                SessionErrorCode::Storage
            };
            Err(
                SessionError::new(code, format!("{}: {}", message, error.message))
                    .with_cause(Arc::new(error)),
            )
        }
    }
}

/// 构造 `invalid_entry` 会话错误(对齐 TS `invalidFile`)。
pub fn invalid_file(path: &str, line: usize, cause: &dyn std::fmt::Display) -> SessionError {
    SessionError::new(
        SessionErrorCode::InvalidEntry,
        format!("Invalid JSONL v4 session {path}: line {line} {cause}"),
    )
}
