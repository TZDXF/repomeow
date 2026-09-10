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
/// 生成始终使用内置 Agent;旧版 config.json 的嵌套 `backend` 字段(内置/本地
/// agent 后端)在读取时兼容:内置后端的模型/思考强度/并发迁移为扁平字段,
/// agent 后端的配置为其 agent 专属,不迁移(回退默认)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiGenerationConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    /// 模型引用(复合值 "providerId/modelId";None = 设置页默认模型)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 思考强度(chat 七档 off..max;None = 模型默认:推理模型中档 / 其余关闭)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// 页面并发数(None/0 = 沿用设置页全局 AI 并发,上限 8)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
}

const fn default_config_version() -> u32 {
    CONFIG_VERSION
}

impl Default for WikiGenerationConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            model: None,
            thinking: None,
            concurrency: None,
        }
    }
}

impl<'de> Deserialize<'de> for WikiGenerationConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// 旧版嵌套后端配置:{ "backend": { "kind": "builtin" | "agent", ... } }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct LegacyBackend {
            #[serde(default)]
            kind: Option<String>,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            thinking: Option<String>,
            #[serde(default)]
            concurrency: Option<usize>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            #[serde(default = "default_config_version")]
            version: u32,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            thinking: Option<String>,
            #[serde(default)]
            concurrency: Option<usize>,
            #[serde(default)]
            backend: Option<LegacyBackend>,
        }
        let helper = Helper::deserialize(deserializer)?;
        let legacy = helper
            .backend
            .and_then(|backend| match backend.kind.as_deref() {
                None | Some("") | Some("builtin") => Some(backend),
                _ => None,
            });
        let (model, thinking, concurrency) = match legacy {
            Some(backend) => (backend.model, backend.thinking, backend.concurrency),
            None => (None, None, None),
        };
        Ok(Self {
            version: helper.version,
            model: helper.model.or(model),
            thinking: helper.thinking.or(thinking),
            concurrency: helper.concurrency.or(concurrency),
        })
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
