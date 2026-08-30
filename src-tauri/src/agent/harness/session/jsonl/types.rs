//! JSONL 会话元数据与 v4 头(对齐 `packages/agent/src/harness/session/jsonl/types.ts`)。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::types::SessionMetadata;

/// JSONL 存储所需的文件系统能力子集(蓝本 `JsonlSessionRepoFileSystem`;
/// Rust 侧直接使用完整 `FileSystem` trait 对象)。
pub type JsonlSessionRepoFileSystem = std::sync::Arc<dyn crate::agent::harness::types::FileSystem>;

/// JSONL 仓库选项(对齐 TS `JsonlSessionRepoOptions`)。
#[derive(Clone)]
pub struct JsonlSessionRepoOptions {
    pub fs: JsonlSessionRepoFileSystem,
    /// 承载 cwd 编码会话目录的根目录。
    pub sessions_root: String,
}

/// JSONL 会话元数据(对齐 TS `JsonlSessionMetadata extends SessionMetadata`)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlSessionMetadata {
    pub id: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub cwd: String,
    pub path: String,
    /// 文件修改时间(Unix 毫秒)。
    pub modified_at: f64,
    /// 3 | 4;本实现只写 v4。
    pub source_format: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_parent_session_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, Value>>,
}

impl From<&JsonlSessionMetadata> for SessionMetadata {
    fn from(metadata: &JsonlSessionMetadata) -> Self {
        SessionMetadata {
            id: metadata.id.clone(),
            created_at: metadata.created_at,
            parent_session_id: metadata.parent_session_id.clone(),
        }
    }
}

/// 创建选项(对齐 TS `JsonlSessionCreateOptions`)。
#[derive(Clone, Debug, Default)]
pub struct JsonlSessionCreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
    pub cwd: String,
    pub metadata: Option<serde_json::Map<String, Value>>,
}

/// 列表选项(对齐 TS `JsonlSessionListOptions`)。
#[derive(Clone, Debug, Default)]
pub struct JsonlSessionListOptions {
    pub cwd: Option<String>,
}

/// JSONL v4 头(对齐 TS `JsonlV4Header`;kind 恒为 "header")。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlV4Header {
    pub kind: String,
    pub version: i64,
    pub id: String,
    pub created_at: i64,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_parent_session_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, Value>>,
}

impl JsonlV4Header {
    pub fn new(id: impl Into<String>, created_at: i64, cwd: impl Into<String>) -> Self {
        Self {
            kind: "header".to_string(),
            version: 4,
            id: id.into(),
            created_at,
            cwd: cwd.into(),
            parent_session_id: None,
            legacy_parent_session_path: None,
            metadata: None,
        }
    }
}
