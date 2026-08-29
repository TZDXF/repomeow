use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticStatus {
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticDiffResult {
    pub engine_version: String,
    pub summary: SemanticDiffSummary,
    pub changes: Vec<SemanticChange>,
    pub binary_changes: Vec<SemanticBinaryChange>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticDiffSummary {
    #[serde(default)]
    pub file_count: usize,
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub modified: usize,
    #[serde(default)]
    pub deleted: usize,
    #[serde(default)]
    pub moved: usize,
    #[serde(default)]
    pub renamed: usize,
    #[serde(default)]
    pub reordered: usize,
    #[serde(default)]
    pub binary: usize,
    #[serde(default)]
    pub orphan: usize,
    #[serde(default)]
    pub total: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticChange {
    pub entity_id: String,
    pub change_type: String,
    pub entity_type: String,
    pub entity_name: String,
    #[serde(default)]
    pub start_line: usize,
    #[serde(default)]
    pub end_line: usize,
    #[serde(default)]
    pub old_start_line: Option<usize>,
    #[serde(default)]
    pub old_end_line: Option<usize>,
    #[serde(default)]
    pub old_entity_name: Option<String>,
    pub file_path: String,
    #[serde(default)]
    pub old_file_path: Option<String>,
    #[serde(default)]
    pub structural_change: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticBinaryChange {
    pub change_type: String,
    pub file_path: String,
    #[serde(default)]
    pub old_file_path: Option<String>,
    pub file_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemCliEnvelope {
    pub summary: SemanticDiffSummary,
    #[serde(default)]
    pub changes: Vec<SemCliChange>,
    #[serde(default)]
    pub binary_changes: Vec<SemanticBinaryChange>,
}

/// sem CLI 的 JSON 含实体前后全文；RepoMeow 的提交详情已有按文件 diff，
/// IPC 只保留定位与分类字段，避免大提交把整份源码再传一次给前端。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SemCliChange {
    pub entity_id: String,
    pub change_type: String,
    pub entity_type: String,
    pub entity_name: String,
    #[serde(default)]
    pub start_line: usize,
    #[serde(default)]
    pub end_line: usize,
    #[serde(default)]
    pub old_start_line: Option<usize>,
    #[serde(default)]
    pub old_end_line: Option<usize>,
    #[serde(default)]
    pub old_entity_name: Option<String>,
    pub file_path: String,
    #[serde(default)]
    pub old_file_path: Option<String>,
    #[serde(default)]
    pub structural_change: Option<bool>,
    #[serde(default, rename = "beforeContent")]
    _before_content: Option<String>,
    #[serde(default, rename = "afterContent")]
    _after_content: Option<String>,
}

impl From<SemCliChange> for SemanticChange {
    fn from(value: SemCliChange) -> Self {
        Self {
            entity_id: value.entity_id,
            change_type: value.change_type,
            entity_type: value.entity_type,
            entity_name: value.entity_name,
            start_line: value.start_line,
            end_line: value.end_line,
            old_start_line: value.old_start_line,
            old_end_line: value.old_end_line,
            old_entity_name: value.old_entity_name,
            file_path: value.file_path,
            old_file_path: value.old_file_path,
            structural_change: value.structural_change,
        }
    }
}
