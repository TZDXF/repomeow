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

/// 对前端稳定的实体引用。sem 各命令的字段风格不一(id / entityId / file /
/// file_path、lines 数组 / start_line 对),统一为 camelCase DTO;实体全文
/// (content / beforeContent / afterContent)一律不进 IPC。
///
/// `entity_id`:能按 sem 规则(`parent_id::name`,根实体为 `file::type::name`)
/// 可靠构造时给出,否则为 null;前端遇到 null 时回退用 name + filePath 查询。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEntityRef {
    #[serde(default)]
    pub entity_id: Option<String>,
    pub name: String,
    pub entity_type: String,
    pub file_path: String,
    #[serde(default)]
    pub start_line: usize,
    #[serde(default)]
    pub end_line: usize,
}

/// 带父子关系的文件内实体(semantic_file_entities 返回项)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticFileEntity {
    #[serde(flatten)]
    pub entity: SemanticEntityRef,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticFileEntitiesResult {
    pub engine_version: String,
    pub file_path: String,
    pub entities: Vec<SemanticFileEntity>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticFindResult {
    pub engine_version: String,
    pub query: String,
    pub results: Vec<SemanticEntityRef>,
    pub truncated: bool,
}

/// callers/refs 返回的分组:目标实体 + 关系项(名称不唯一时可能有多组)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRelationGroup {
    pub entity: SemanticEntityRef,
    pub related: Vec<SemanticEntityRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRelationResult {
    pub engine_version: String,
    pub groups: Vec<SemanticRelationGroup>,
    pub truncated: bool,
}

/// 传递影响实体:在实体引用上附带相对目标实体的深度。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticImpactedEntity {
    #[serde(flatten)]
    pub entity: SemanticEntityRef,
    #[serde(default)]
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticImpactResult {
    pub engine_version: String,
    pub entity: SemanticEntityRef,
    pub dependencies: Vec<SemanticEntityRef>,
    pub dependents: Vec<SemanticEntityRef>,
    pub affected: Vec<SemanticImpactedEntity>,
    pub tests: Vec<SemanticEntityRef>,
    pub total: usize,
    pub depth: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticBlameEntry {
    pub name: String,
    pub entity_type: String,
    #[serde(default)]
    pub start_line: usize,
    #[serde(default)]
    pub end_line: usize,
    pub author: String,
    pub commit: String,
    pub date: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticFileBlameResult {
    pub engine_version: String,
    pub file_path: String,
    pub entries: Vec<SemanticBlameEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEntityLogChange {
    pub change_type: String,
    #[serde(default)]
    pub structural_change: Option<bool>,
    #[serde(default)]
    pub file_path: String,
    pub commit_sha: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticEntityLogResult {
    pub engine_version: String,
    pub entity: String,
    pub entity_type: String,
    pub file_path: String,
    pub changes: Vec<SemanticEntityLogChange>,
    pub truncated: bool,
}

/// sem context 的单条上下文。`content` 是源码片段:仅在用户显式触发后经
/// IPC 返回,不落库、不写日志。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticContextEntry {
    pub entity_id: String,
    pub name: String,
    pub entity_type: String,
    pub file_path: String,
    pub role: String,
    #[serde(default)]
    pub tokens: usize,
    pub content: String,
}

/// 因预算被省略的一组实体(按角色聚合计数)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticContextOmitted {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub entities: usize,
    #[serde(default)]
    pub tests: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticContextResult {
    pub engine_version: String,
    pub entity: String,
    pub entity_id: String,
    pub budget: usize,
    pub total_tokens: usize,
    pub truncated: bool,
    pub target_omitted: bool,
    pub entries: Vec<SemanticContextEntry>,
    pub omitted: Vec<SemanticContextOmitted>,
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
    pub(super) before_content: Option<String>,
    #[serde(default, rename = "afterContent")]
    pub(super) after_content: Option<String>,
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
