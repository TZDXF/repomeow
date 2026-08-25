use serde::{Deserialize, Serialize};

pub(super) const CONFIG_VERSION: u32 = 1;

/// 触发 wiki git 提交的操作类型(决定提交信息措辞;序列化为 "generate"/"update"/"page")
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WikiCommitKind {
    Generate,
    Update,
    Page,
}

/// 大纲中的单个页面条目(meta.json 的 outline 元素)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiOutlinePage {
    pub id: String,
    /// 页面文件名(pages/ 下,如 `01-overview.md`)
    pub file: String,
    pub title: String,
    /// 该页覆盖内容的简述(大纲阶段产出,单页生成时注入 prompt)
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub importance: String,
    #[serde(default)]
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub related_pages: Vec<String>,
}

/// wiki 元信息;`generated_at` 与 `version` 由 save_wiki_meta 覆写,前端无需填
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiMeta {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub outline: Vec<WikiOutlinePage>,
    /// 生成后端标识("builtin" / "acp:<agentId>");旧 meta 缺省视为内置。
    /// 前端手动增量更新遇后端切换时退化为整本重生成
    #[serde(default)]
    pub generator: Option<String>,
}

/// 单个项目的 Wiki 生成配置，独立保存在该项目 Wiki 目录的 config.json。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiGenerationConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub backend: crate::commands::ai::WikiGenerationBackend,
}

const fn default_config_version() -> u32 {
    CONFIG_VERSION
}

impl Default for WikiGenerationConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            backend: crate::commands::ai::WikiGenerationBackend::Builtin,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiPageData {
    pub id: String,
    pub file: String,
    pub title: String,
    pub section: Option<String>,
    pub importance: String,
    pub relevant_files: Vec<String>,
    pub related_pages: Vec<String>,
    /// 页面 Markdown 正文;文件缺失时为空串(前端显示占位)
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiData {
    pub meta: WikiMeta,
    pub pages: Vec<WikiPageData>,
    /// 生成时的 HEAD 与当前 HEAD 不一致(代码已更新,wiki 可能过时)
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiManifest {
    pub path: String,
    pub content: String,
}

/// 结构阶段的输入:过滤后的文件树 + README + 根目录清单文件 + 当前 HEAD
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiContext {
    pub file_tree: String,
    /// 过滤后的完整文件清单(/ 分隔相对路径,不折叠),后端用于校验大纲标注的相关文件
    pub paths: Vec<String>,
    pub file_count: usize,
    pub tree_truncated: bool,
    pub readme: Option<String>,
    pub manifests: Vec<WikiManifest>,
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiFileContent {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiChangedFiles {
    pub files: Vec<String>,
    /// 当前 HEAD(增量更新成功后回写 meta)
    pub head_sha: Option<String>,
}
