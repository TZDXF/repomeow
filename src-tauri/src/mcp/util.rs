use std::env;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::Db;
use crate::error::AppError;
use crate::path_util::clean_str;
use crate::APP_DATA_DIR_NAME;
use rmcp::model::CallToolResult;
use serde_json::json;

pub(super) const PROJECTS_DB_FILE: &str = "projects.db";
pub(super) const DATA_DIR_ENV: &str = "REPOMEOW_DATA_DIR";
pub(super) const WIKI_DIR_NAME: &str = "wiki";
pub(super) const WIKI_META_FILE: &str = "meta.json";

#[derive(Debug)]
pub(super) struct ToolFailure {
    pub(super) code: String,
    pub(super) message: String,
    pub(super) detail: Option<String>,
}

impl ToolFailure {
    pub(super) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    pub(super) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(super) fn from_app(message: impl Into<String>, error: AppError) -> Self {
        Self {
            code: error.code().to_string(),
            message: message.into(),
            detail: Some(error.to_string()),
        }
    }

    pub(super) fn into_result(self) -> CallToolResult {
        CallToolResult::structured_error(json!({
            "code": self.code,
            "message": self.message,
            "detail": self.detail,
        }))
    }
}


pub(super) fn repomeow_data_root() -> Result<PathBuf, ToolFailure> {
    if let Some(path) = env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    home_dir()
        .map(|home| home.join(APP_DATA_DIR_NAME))
        .ok_or_else(|| ToolFailure::new("home_directory_unavailable", "无法确定当前用户主目录"))
}

// ── 共享辅助 ──────────────────────────────────────────────────────────

/// 实体参数:含 "::" 视为 entityId 精确匹配,否则视为实体名;空白输入为 None。
pub(super) fn split_entity_token(value: &str) -> Option<(Option<String>, Option<String>)> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("::") {
        Some((Some(trimmed.to_string()), None))
    } else {
        Some((None, Some(trimmed.to_string())))
    }
}

/// 按 UTF-8 边界截断;返回 (文本, 是否截断)。
pub(super) fn truncate_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

pub(super) fn data_root_or_default(data_root: Option<&Path>) -> Result<PathBuf, ToolFailure> {
    match data_root {
        Some(root) => Ok(root.to_path_buf()),
        None => repomeow_data_root(),
    }
}

pub(super) fn open_db(data_root: &Path) -> Result<Db, ToolFailure> {
    Db::open(&data_root.join(PROJECTS_DB_FILE))
        .map_err(|error| ToolFailure::from_app("打开 RepoMeow 数据库失败", error))
}

/// 按登记目录(归一化后)定位未归档项目 id。
pub(super) fn resolve_project_id(conn: &Connection, directory: &str) -> Result<Option<i64>, ToolFailure> {
    let path = clean_str(directory);
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1 AND archived_at IS NULL",
        params![path],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| {
        ToolFailure::new("db_query_failed", "查询项目失败").with_detail(error.to_string())
    })
}

pub(super) fn require_project_id(conn: &Connection, directory: &str) -> Result<i64, ToolFailure> {
    resolve_project_id(conn, directory)?.ok_or_else(|| {
        ToolFailure::new("project_not_found", "该项目未在 RepoMeow 登记或已归档")
            .with_detail(clean_str(directory))
    })
}



#[cfg(windows)]
pub(super) fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE").map(PathBuf::from).or_else(|| {
        let drive = env::var_os("HOMEDRIVE")?;
        let path = env::var_os("HOMEPATH")?;
        Some(PathBuf::from(drive).join(path))
    })
}

#[cfg(not(windows))]
pub(super) fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
