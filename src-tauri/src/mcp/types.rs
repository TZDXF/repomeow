use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommitCodeInput {
    /// Git 仓库目录。可以使用绝对路径，提交范围始终以仓库根目录为准。
    pub directory: String,
    /// Git 提交信息，不能为空。
    pub message: String,
    /// 可选的仓库相对路径列表。省略时提交全部变更（含未跟踪文件）。
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitCodeOutput {
    pub directory: String,
    pub commit_hash: String,
    pub short_hash: String,
    pub branch: Option<String>,
    pub committed_files: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetWikiDirectoryInput {
    /// RepoMeow 中项目登记使用的目录。路径会按 RepoMeow 的规则归一化后定位 Wiki。
    pub project_directory: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiDirectoryOutput {
    pub project_directory: String,
    pub wiki_directory: String,
    pub meta_path: String,
    pub meta: Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusInput {
    /// Git 仓库目录(仓库内任意路径均可)。
    pub directory: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadWikiPageInput {
    /// RepoMeow 中项目登记使用的目录。
    pub project_directory: String,
    /// 要读取的页面 id(来自 list_wiki_pages 的大纲清单)。
    pub page_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiPagesOutput {
    pub project_directory: String,
    /// 生成时的 HEAD 与当前 HEAD 不一致(代码已更新,Wiki 可能过时)。
    pub stale: bool,
    pub generated_at: String,
    pub head_sha: Option<String>,
    pub generator: Option<String>,
    pub model: String,
    pub pages: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiPageOutput {
    pub id: String,
    pub title: String,
    pub file: String,
    pub stale: bool,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SemFindInput {
    /// Git 仓库目录(仓库内任意路径均可)。
    pub directory: String,
    /// 搜索关键词:实体名或其一部分,如 "debounce"、"WikiGenKernel"。
    pub query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SemEntityInput {
    /// Git 仓库目录(仓库内任意路径均可)。
    pub directory: String,
    /// 实体名或 entityId(形如 src/a.ts::function::run,含 "::" 时按 entityId 精确匹配)。
    pub entity: String,
    /// 实体所在文件的仓库相对路径(/ 分隔),重名时用于消歧。
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SemContextInput {
    /// Git 仓库目录(仓库内任意路径均可)。
    pub directory: String,
    /// 实体名或 entityId(含 "::" 的串视为 entityId)。
    pub entity: String,
    /// 实体所在文件的仓库相对路径(/ 分隔),重名时用于消歧。
    pub file_path: Option<String>,
    /// 上下文预算(token 数,500-4000),缺省 2000。
    pub budget: Option<u32>,
    /// 关系扩展跳数(0-3),缺省 1。
    pub hops: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadProjectFileInput {
    /// 项目目录(读取范围以该目录为根,拒绝越界与符号链接逃逸)。
    pub project_directory: String,
    /// 项目内相对路径(/ 分隔),如 src/lib/ai.ts。
    pub path: String,
    /// 起始行(1-based),默认 1。
    pub offset_line: Option<u64>,
    /// 最多返回行数,默认 400,上限 5000。
    pub max_lines: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileOutput {
    pub path: String,
    /// 文件总行数(在 512KB 预览上限内)。
    pub total_lines: usize,
    pub start_line: u64,
    pub end_line: u64,
    /// 内容带 `N: ` 行号前缀(1-based)。
    pub content: String,
    /// 后面还有更多行(用 offset_line=endLine+1 续读)。
    pub has_more: bool,
    /// 文件超过 512KB 预览上限,尾部被截断。
    pub preview_truncated: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListReportsInput {
    /// 仅列出该项目的报告;省略时列出全部项目的报告。
    pub project_directory: Option<String>,
    /// 返回条数(1-50),默认 10。
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDirectoryInput {
    /// RepoMeow 中项目登记使用的目录。
    pub project_directory: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GenerateReportInput {
    /// 参与统计的项目目录列表(必须是 RepoMeow 已登记且未归档的项目)。
    pub project_directories: Vec<String>,
    /// 报告类型:daily 日报 / weekly 周报。
    pub period_type: String,
    /// 起始日期 YYYY-MM-DD;缺省 daily=今天、weekly=6 天前。
    pub date_from: Option<String>,
    /// 结束日期 YYYY-MM-DD;缺省今天。
    pub date_to: Option<String>,
    /// 提交作者范围:all 全部(默认)/ me 仅当前 git 用户。
    pub author_mode: Option<String>,
    /// 报告语言:zh-CN(默认)/ en-US。
    pub language: Option<String>,
}
