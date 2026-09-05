//! 资源库模块自包含错误类型。
//!
//! 序列化形状与 `AppError` 完全一致(`{code, message}`),前端沿用同一条
//! `errors.<code>` i18n 通道。模块不修改全局 `error.rs`;下列新错误码建议
//! 后续并入 `crate::error::ErrorCode`,并同步补 i18n 词条。

use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;

use crate::error::AppError;

/// 资源库专属错误码(小写蛇形,与前端 i18n `errors.*` 键对应)。
/// 通用 io/json/git 底层错误不走这里,由 `RlError::App` 透传既有码。
pub mod codes {
    pub const LOCKED: &str = "resource_library_locked";
    pub const PASSWORD_INVALID: &str = "resource_library_password_invalid";
    pub const PASSWORD_REQUIRED: &str = "resource_library_password_required";
    pub const ALREADY_ENCRYPTED: &str = "resource_library_already_encrypted";
    pub const NOT_ENCRYPTED: &str = "resource_library_not_encrypted";
    pub const CORRUPT: &str = "resource_library_corrupt";
    pub const SKILL_NOT_FOUND: &str = "resource_library_skill_not_found";
    pub const SKILL_NAME_REQUIRED: &str = "resource_library_skill_name_required";
    pub const SKILL_NAME_CONFLICT: &str = "resource_library_skill_name_conflict";
    pub const GROUP_NOT_FOUND: &str = "resource_library_group_not_found";
    pub const GROUP_NAME_REQUIRED: &str = "resource_library_group_name_required";
    pub const GROUP_NAME_CONFLICT: &str = "resource_library_group_name_conflict";
    pub const GROUP_COLOR_INVALID: &str = "resource_library_group_color_invalid";
    pub const DIRECTORY_INVALID: &str = "resource_library_directory_invalid";
    pub const DIRECTORY_CONFLICT: &str = "resource_library_directory_conflict";
    pub const MCP_NOT_FOUND: &str = "resource_library_mcp_not_found";
    pub const MCP_NAME_REQUIRED: &str = "resource_library_mcp_name_required";
    pub const MCP_NAME_CONFLICT: &str = "resource_library_mcp_name_conflict";
    pub const MCP_TRANSPORT_INVALID: &str = "resource_library_mcp_transport_invalid";
    pub const MCP_COMMAND_REQUIRED: &str = "resource_library_mcp_command_required";
    pub const MCP_URL_REQUIRED: &str = "resource_library_mcp_url_required";
    #[cfg(test)]
    pub const IMPORT_CONFLICT: &str = "resource_library_import_conflict";
    pub const NOT_DIVERGED: &str = "resource_library_not_diverged";
    pub const REMOTE_REQUIRED: &str = "resource_library_remote_required";
    pub const DIRECTION_INVALID: &str = "resource_library_direction_invalid";
    pub const NOT_INITIALIZED: &str = "resource_library_not_initialized";
    pub const DIRTY: &str = "resource_library_dirty";
    #[cfg(test)]
    pub const BEHIND: &str = "resource_library_behind";
    pub const DIVERGED: &str = "resource_library_diverged";
    pub const MARKETPLACE_UNAVAILABLE: &str = "resource_library_marketplace_unavailable";
    pub const MARKETPLACE_INVALID_RESPONSE: &str = "resource_library_marketplace_invalid_response";
    pub const MARKETPLACE_RESPONSE_TOO_LARGE: &str =
        "resource_library_marketplace_response_too_large";
    pub const MARKETPLACE_MODE_INVALID: &str = "resource_library_marketplace_mode_invalid";
    pub const MARKETPLACE_QUERY_REQUIRED: &str = "resource_library_marketplace_query_required";
    pub const MARKETPLACE_SOURCE_INVALID: &str = "resource_library_marketplace_source_invalid";
    pub const MARKETPLACE_ID_INVALID: &str = "resource_library_marketplace_id_invalid";
    pub const MARKETPLACE_SKILL_INVALID: &str = "resource_library_marketplace_skill_invalid";
    pub const IMPORT_SOURCE_INVALID: &str = "resource_library_import_source_invalid";
    pub const SKILL_IMPORT_EMPTY: &str = "resource_library_skill_import_empty";
    pub const ARCHIVE_INVALID: &str = "resource_library_archive_invalid";
    pub const ARCHIVE_TOO_LARGE: &str = "resource_library_archive_too_large";
    pub const URL_INVALID: &str = "resource_library_url_invalid";
    pub const DOWNLOAD_FAILED: &str = "resource_library_download_failed";
}

/// 资源库错误。三类构成:
/// - `Io`:底层 io,序列化为 `io_error`
/// - `Corrupt`:库数据文件损坏(JSON/AEAD/UTF-8 校验失败),`message` 携带上下文
/// - `App`:透传既有 `AppError`(git 底层等),保留原始错误码
/// - `Coded`:面向用户、需前端 i18n 的错误,`message` 仅含技术上下文
#[derive(Debug, thiserror::Error)]
pub enum RlError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Corrupt(String),
    #[error("{0}")]
    App(#[from] AppError),
    #[error("code={code} message={message}")]
    Coded { code: &'static str, message: String },
}

impl RlError {
    pub fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }

    /// 数据文件损坏:附上文件/操作上下文与底层原因
    pub fn corrupt(ctx: impl std::fmt::Display, detail: impl std::fmt::Display) -> Self {
        Self::Corrupt(format!("{ctx}: {detail}"))
    }

    /// 错误码(恒有值;App 透传码与 Db/Io 映射与 AppError 一致)
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io_error",
            Self::Corrupt(_) => codes::CORRUPT,
            Self::App(e) => e.code(),
            Self::Coded { code, .. } => code,
        }
    }

    /// 可展示的技术上下文(message);面向用户文案由前端 i18n 渲染
    pub fn message(&self) -> String {
        match self {
            Self::Io(e) => e.to_string(),
            Self::Corrupt(m) => m.clone(),
            Self::App(e) => match e {
                AppError::Db(x) => x.to_string(),
                AppError::Io(x) => x.to_string(),
                AppError::Coded { message, .. } => message.clone(),
            },
            Self::Coded { message, .. } => message.clone(),
        }
    }
}

/// Tauri 命令错误序列化:与 AppError 同构,前端 `translateCommandError` 直接可用
impl Serialize for RlError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("RlError", 2)?;
        s.serialize_field("code", &self.code())?;
        s.serialize_field("message", &self.message())?;
        s.end()
    }
}

pub type RlResult<T> = Result<T, RlError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_emits_code_and_message() {
        let raw = RlError::coded(codes::LOCKED, "");
        let json = serde_json::to_value(&raw).unwrap();
        assert_eq!(json["code"], "resource_library_locked");
        assert_eq!(json["message"], "");
    }

    #[test]
    fn app_passthrough_keeps_original_code() {
        let raw = RlError::App(AppError::coded(crate::error::ErrorCode::GitAuthFailed, "x"));
        let json = serde_json::to_value(&raw).unwrap();
        assert_eq!(json["code"], "git_auth_failed");
        assert_eq!(json["message"], "x");
    }

    #[test]
    fn corrupt_maps_to_corrupt_code() {
        let raw = RlError::corrupt("skills.json", "expected value");
        assert_eq!(raw.code(), "resource_library_corrupt");
        assert_eq!(raw.message(), "skills.json: expected value");
    }

    #[test]
    fn io_maps_to_io_error_code() {
        let raw: RlError = std::io::Error::new(std::io::ErrorKind::NotFound, "x").into();
        assert_eq!(raw.code(), "io_error");
    }
}
