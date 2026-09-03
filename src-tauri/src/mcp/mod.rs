use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::Local;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{
    schemars, tool, tool_handler, tool_router, transport::stdio, ServerHandler, ServiceExt,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::ai::{mcp_generate_and_save_report, GenerateAndSaveReportRequest};
use crate::commands::files::read_file_preview;
use crate::commands::git::{commit_blocking, run_git};
use crate::commands::report::list_report_history_impl;
use crate::commands::wiki::{load_wiki_at, wiki_dir_in};
use crate::commands::{script, semantic};
use crate::db::Db;
use crate::error::AppError;
use crate::path_util::{clean_str, to_forward_slash_str};
use crate::APP_DATA_DIR_NAME;

const WIKI_DIR_NAME: &str = "wiki";
const WIKI_META_FILE: &str = "meta.json";
const PROJECTS_DB_FILE: &str = "projects.db";
const DATA_DIR_ENV: &str = "REPOMEOW_DATA_DIR";
const SETTINGS_FILE: &str = "settings.json";
const GIT_COMMIT_ENABLED_KEY: &str = "mcpGitCommitEnabled";
const WIKI_ENABLED_KEY: &str = "mcpWikiEnabled";
const SEM_ENABLED_KEY: &str = "mcpSemEnabled";
const PROJECT_ENABLED_KEY: &str = "mcpProjectEnabled";
const REPORT_ENABLED_KEY: &str = "mcpReportEnabled";

/// read_wiki_page 单页正文字节上限(对齐 chat 工具)。
const WIKI_PAGE_MAX_BYTES: usize = 24 * 1024;
/// read_project_file 默认/最大返回行数(对齐 chat 工具)。
const READ_FILE_DEFAULT_LINES: u64 = 400;
const READ_FILE_MAX_LINES: u64 = 5000;
/// generate_report 返回正文的字节上限(对齐 chat 工具)。
const REPORT_RESULT_MAX_BYTES: usize = 4 * 1024;

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

#[derive(Debug)]
struct ToolFailure {
    code: String,
    message: String,
    detail: Option<String>,
}

impl ToolFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn from_app(message: impl Into<String>, error: AppError) -> Self {
        Self {
            code: error.code().to_string(),
            message: message.into(),
            detail: Some(error.to_string()),
        }
    }

    fn into_result(self) -> CallToolResult {
        CallToolResult::structured_error(json!({
            "code": self.code,
            "message": self.message,
            "detail": self.detail,
        }))
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct McpToolGroups {
    git_commit: bool,
    wiki: bool,
    sem: bool,
    project: bool,
    report: bool,
}

#[derive(Debug, Clone)]
pub struct RepoMeowMcpServer {
    tool_router: ToolRouter<Self>,
}

impl Default for RepoMeowMcpServer {
    fn default() -> Self {
        Self::new(McpToolGroups::default())
    }
}

#[tool_router(router = tool_router)]
impl RepoMeowMcpServer {
    fn new(groups: McpToolGroups) -> Self {
        let mut tool_router = Self::tool_router();
        if !groups.git_commit {
            tool_router.disable_route("commit_code");
            tool_router.disable_route("get_git_status");
        }
        if !groups.wiki {
            tool_router.disable_route("get_wiki_directory");
            tool_router.disable_route("list_wiki_pages");
            tool_router.disable_route("read_wiki_page");
        }
        if !groups.sem {
            tool_router.disable_route("sem_find");
            tool_router.disable_route("sem_context");
            tool_router.disable_route("sem_relations");
            tool_router.disable_route("sem_diff");
        }
        if !groups.project {
            tool_router.disable_route("read_project_file");
            tool_router.disable_route("list_reports");
            tool_router.disable_route("list_custom_commands");
        }
        if !groups.report {
            tool_router.disable_route("generate_report");
        }
        Self { tool_router }
    }

    #[tool(
        name = "commit_code",
        description = "在指定 Git 仓库中创建代码提交。files 省略时提交全部变更（含未跟踪文件）；传入时仅提交指定的仓库相对路径。",
        annotations(
            title = "提交代码",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn commit_code(
        &self,
        Parameters(input): Parameters<CommitCodeInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let result = tokio::task::spawn_blocking(move || commit_code_impl(input)).await;
        Ok(match result {
            Ok(Ok(output)) => CallToolResult::structured(json!(output)),
            Ok(Err(error)) => error.into_result(),
            Err(error) => ToolFailure::new("git_task_failed", "代码提交任务执行失败")
                .with_detail(error.to_string())
                .into_result(),
        })
    }

    #[tool(
        name = "get_wiki_directory",
        description = "获取指定项目已经生成完成的 RepoMeow Wiki 目录和 meta.json 元数据。未生成 Wiki 时返回错误。",
        annotations(
            title = "获取 Wiki 目录",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_wiki_directory(
        &self,
        Parameters(input): Parameters<GetWikiDirectoryInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match get_wiki_directory_impl(input, None) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }

    #[tool(
        name = "get_git_status",
        description = "获取指定 Git 仓库的状态摘要:当前分支、与上游的领先/落后提交数、暂存/未暂存修改/未跟踪/冲突文件数、最后抓取与提交时间。非 git 目录返回 isRepo=false。",
        annotations(
            title = "Git 状态摘要",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_git_status(
        &self,
        Parameters(input): Parameters<GitStatusInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let directory = clean_str(&input.directory);
        let result = tokio::task::spawn_blocking(move || {
            crate::commands::git::status(&directory)
        })
        .await;
        Ok(match result {
            Ok(Ok(status)) => CallToolResult::structured(json!(status)),
            Ok(Err(error)) => ToolFailure::from_app("读取 Git 状态失败", error).into_result(),
            Err(error) => ToolFailure::new("git_task_failed", "Git 状态任务执行失败")
                .with_detail(error.to_string())
                .into_result(),
        })
    }

    #[tool(
        name = "list_wiki_pages",
        description = "列出指定项目已由 RepoMeow 生成的 Wiki 大纲:每页的 id/标题/简介/分区/来源文件,以及 Wiki 是否已落后于最新代码(stale)。未生成 Wiki 时返回错误;读某页正文用 read_wiki_page。",
        annotations(
            title = "Wiki 大纲",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_wiki_pages(
        &self,
        Parameters(input): Parameters<ProjectDirectoryInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match list_wiki_pages_impl(input, None) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }

    #[tool(
        name = "read_wiki_page",
        description = "读取指定项目 Wiki 中某一页的正文 Markdown(超长截断,truncated 标记)。页面 id 来自 list_wiki_pages;stale=true 表示 Wiki 落后于最新代码,内容可能过时。",
        annotations(
            title = "读 Wiki 页面",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn read_wiki_page(
        &self,
        Parameters(input): Parameters<ReadWikiPageInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match read_wiki_page_impl(input, None) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }

    #[tool(
        name = "sem_find",
        description = "按名称语义搜索仓库内的代码实体(函数/类/接口/结构体等)。回答「XX 在哪里实现」「项目里有没有 XX」前先用它定位,再用 sem_context 查看上下文。返回命中实体列表(entityId/name/entityType/filePath/startLine/endLine)与 truncated 标记。",
        annotations(
            title = "语义搜索",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn sem_find(
        &self,
        Parameters(input): Parameters<SemFindInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let directory = clean_str(&input.directory);
        let query = input.query.trim().to_string();
        if query.is_empty() {
            return Ok(ToolFailure::new("invalid_query", "搜索关键词不能为空").into_result());
        }
        Ok(
            match semantic::mcp_semantic_find_entities(directory, query).await {
                Ok(result) => CallToolResult::structured(json!(result)),
                Err(error) => ToolFailure::from_app("语义搜索失败", error).into_result(),
            },
        )
    }

    #[tool(
        name = "sem_context",
        description = "查看某个代码实体的语义上下文:实体源码摘要与按调用/引用关系扩展出的相关实体。在 sem_find 定位到实体后,用它理解实现细节。",
        annotations(
            title = "实体上下文",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn sem_context(
        &self,
        Parameters(input): Parameters<SemContextInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let directory = clean_str(&input.directory);
        let Some((entity_id, entity_name)) = split_entity_token(&input.entity) else {
            return Ok(ToolFailure::new("invalid_entity", "实体名不能为空").into_result());
        };
        Ok(
            match semantic::mcp_semantic_entity_context(
                directory,
                entity_id,
                entity_name,
                input.file_path,
                input.budget.map(|value| value as usize),
                input.hops.map(|value| value as usize),
            )
            .await
            {
                Ok(result) => CallToolResult::structured(json!(result)),
                Err(error) => ToolFailure::from_app("读取实体上下文失败", error).into_result(),
            },
        )
    }

    #[tool(
        name = "sem_relations",
        description = "查询某实体的直接调用方(callers)与引用点(refs),两者合并返回。用于回答「谁调用了 XX」「改动 XX 会影响哪些地方」。",
        annotations(
            title = "调用与引用",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn sem_relations(
        &self,
        Parameters(input): Parameters<SemEntityInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let directory = clean_str(&input.directory);
        let Some((entity_id, entity_name)) = split_entity_token(&input.entity) else {
            return Ok(ToolFailure::new("invalid_entity", "实体名不能为空").into_result());
        };
        Ok(
            match semantic::mcp_semantic_entity_relations(
                directory,
                entity_id,
                entity_name,
                input.file_path,
            )
            .await
            {
                Ok((callers, refs)) => CallToolResult::structured(json!({
                    "callers": callers,
                    "refs": refs,
                })),
                Err(error) => ToolFailure::from_app("查询实体关系失败", error).into_result(),
            },
        )
    }

    #[tool(
        name = "sem_diff",
        description = "汇总仓库当前未提交的代码变更(实体级结构化差异摘要:新增/修改/删除的函数、类等,含结构性/外观性标记)。回答「当前改了什么」「最近在做什么」前先调用。",
        annotations(
            title = "未提交变更",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn sem_diff(
        &self,
        Parameters(input): Parameters<GitStatusInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let directory = clean_str(&input.directory);
        Ok(
            match semantic::mcp_semantic_worktree_diff(directory).await {
                Ok(result) => CallToolResult::structured(json!(result)),
                Err(error) => ToolFailure::from_app("读取未提交变更失败", error).into_result(),
            },
        )
    }

    #[tool(
        name = "read_project_file",
        description = "读取项目内单个文本文件的指定行区间(content 带 1-based 行号前缀)。只读;path 必须是项目内相对路径(/ 分隔),拒绝越界与符号链接逃逸;二进制文件返回错误。hasMore=true 时用 offsetLine=endLine+1 续读。",
        annotations(
            title = "读项目文件",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn read_project_file(
        &self,
        Parameters(input): Parameters<ReadProjectFileInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match read_project_file_impl(input) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }

    #[tool(
        name = "list_reports",
        description = "列出 RepoMeow 已生成的日报/周报历史(按生成时间倒序):时间范围、类型、涉及项目、提交数、历史 id。可用 projectDirectory 限定单个项目。",
        annotations(
            title = "报告历史",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_reports(
        &self,
        Parameters(input): Parameters<ListReportsInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match list_reports_impl(input, None) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }

    #[tool(
        name = "list_custom_commands",
        description = "列出指定项目在 RepoMeow 中登记的自定义命令(名称/命令文本/描述),这些命令可在终端一键执行,是了解项目构建/运行方式的捷径。",
        annotations(
            title = "自定义命令清单",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_custom_commands(
        &self,
        Parameters(input): Parameters<ProjectDirectoryInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match list_custom_commands_impl(input, None) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }

    #[tool(
        name = "generate_report",
        description = "为一个或多个已登记项目生成日报/周报:汇总指定时间范围的 git 提交,调用 AI 生成中文正文并保存到 RepoMeow 报告历史(同步等待,通常十几秒到一分钟;会消耗用户在 RepoMeow 配置的 AI 额度)。范围内无提交时不生成。",
        annotations(
            title = "生成日报/周报",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn generate_report(
        &self,
        Parameters(input): Parameters<GenerateReportInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match generate_report_impl(input, None).await {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RepoMeowMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("repomeow-mcp", env!("CARGO_PKG_VERSION"))
                    .with_title("RepoMeow MCP")
                    .with_description("RepoMeow 的 Git、项目 Wiki、代码语义分析、项目数据与报告服务"),
            )
            .with_instructions(
                "按启用的工具组提供工具:git(get_git_status 查状态,commit_code 建提交);\
                 wiki(list_wiki_pages 列大纲,read_wiki_page 读页面,get_wiki_directory 取目录与 meta.json);\
                 sem(sem_find 搜实体,sem_context 看上下文,sem_relations 查调用与引用,sem_diff 看未提交变更);\
                 project(read_project_file 读文件,list_reports 列报告历史,list_custom_commands 列自定义命令);\
                 report(generate_report 生成日报/周报并落库)。\
                 工具组在 RepoMeow 设置页开关,未启用的工具不出现在列表中。",
            )
    }
}

pub fn is_mcp_mode() -> bool {
    env::args_os().skip(1).any(|arg| arg == "--mcp")
}

pub fn serve_stdio_blocking() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(serve_stdio())
}

async fn serve_stdio() -> anyhow::Result<()> {
    let data_root = repomeow_data_root().map_err(|error| anyhow::anyhow!(error.message))?;
    let groups = load_tool_groups(&data_root);
    let service = RepoMeowMcpServer::new(groups).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn load_tool_groups(data_root: &Path) -> McpToolGroups {
    let Ok(raw) = fs::read_to_string(data_root.join(SETTINGS_FILE)) else {
        return McpToolGroups::default();
    };
    let Ok(settings) = serde_json::from_str::<Value>(&raw) else {
        return McpToolGroups::default();
    };
    McpToolGroups {
        git_commit: setting_bool(&settings, GIT_COMMIT_ENABLED_KEY),
        wiki: setting_bool(&settings, WIKI_ENABLED_KEY),
        sem: setting_bool(&settings, SEM_ENABLED_KEY),
        project: setting_bool(&settings, PROJECT_ENABLED_KEY),
        report: setting_bool(&settings, REPORT_ENABLED_KEY),
    }
}

fn setting_bool(settings: &Value, key: &str) -> bool {
    match settings.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => value == "true",
        _ => false,
    }
}

fn commit_code_impl(input: CommitCodeInput) -> Result<CommitCodeOutput, ToolFailure> {
    let directory = input.directory.trim();
    if directory.is_empty() || !Path::new(directory).is_dir() {
        return Err(ToolFailure::new(
            "invalid_directory",
            "代码提交目录不存在或不是文件夹",
        ));
    }
    if input.message.trim().is_empty() {
        return Err(ToolFailure::new(
            "git_commit_message_required",
            "Git 提交信息不能为空",
        ));
    }

    let root_output = run_git(directory, &["rev-parse", "--show-toplevel"])
        .map_err(|error| ToolFailure::from_app("无法定位 Git 仓库根目录", error))?;
    let root = String::from_utf8_lossy(&root_output.stdout)
        .trim()
        .to_string();
    if root.is_empty() {
        return Err(ToolFailure::new(
            "not_git_repository",
            "指定目录不是有效的 Git 工作区",
        ));
    }
    let root = clean_str(&root);
    let selected_files = normalize_commit_paths(input.files)?;

    let pathspecs = selected_files.as_ref().map(|paths| {
        paths
            .iter()
            .map(|path| format!(":(literal){path}"))
            .collect()
    });
    let status = commit_blocking(
        &root,
        input.message.trim(),
        selected_files.is_none(),
        pathspecs,
    );

    let status = status.map_err(|error| ToolFailure::from_app("代码提交失败", error))?;
    let hash = git_output(&root, &["rev-parse", "HEAD"], "读取提交哈希失败")?;
    let short_hash = git_output(
        &root,
        &["rev-parse", "--short", "HEAD"],
        "读取短提交哈希失败",
    )?;
    let committed_files = committed_files(&root)?;

    Ok(CommitCodeOutput {
        directory: root,
        commit_hash: hash,
        short_hash,
        branch: status.branch,
        committed_files,
    })
}

fn normalize_commit_paths(files: Option<Vec<String>>) -> Result<Option<Vec<String>>, ToolFailure> {
    let Some(files) = files else {
        return Ok(None);
    };
    if files.is_empty() {
        return Err(ToolFailure::new(
            "git_paths_required",
            "files 已提供时至少需要包含一个文件路径",
        ));
    }

    let mut normalized = Vec::with_capacity(files.len());
    for raw in files {
        let trimmed = raw.trim();
        let forward = to_forward_slash_str(trimmed);
        let looks_like_drive_path = forward.as_bytes().get(1) == Some(&b':');
        let invalid_component = Path::new(trimmed)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)));
        let invalid_forward_component = forward
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
        if trimmed.is_empty()
            || trimmed.contains('\0')
            || Path::new(trimmed).is_absolute()
            || forward.starts_with('/')
            || looks_like_drive_path
            || invalid_component
            || invalid_forward_component
        {
            return Err(ToolFailure::new(
                "invalid_file_path",
                format!("提交文件必须是仓库内的相对路径：{raw}"),
            ));
        }
        if !normalized.contains(&forward) {
            normalized.push(forward);
        }
    }
    Ok(Some(normalized))
}

fn git_output(root: &str, args: &[&str], message: &str) -> Result<String, ToolFailure> {
    let output = run_git(root, args).map_err(|error| ToolFailure::from_app(message, error))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn committed_files(root: &str) -> Result<Vec<String>, ToolFailure> {
    let output = run_git(
        root,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "HEAD",
        ],
    )
    .map_err(|error| ToolFailure::from_app("读取本次提交文件失败", error))?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| to_forward_slash_str(&String::from_utf8_lossy(path)))
        .collect())
}

fn get_wiki_directory_impl(
    input: GetWikiDirectoryInput,
    data_root: Option<&Path>,
) -> Result<WikiDirectoryOutput, ToolFailure> {
    let project_directory = clean_str(&input.project_directory);
    if project_directory.trim().is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }

    let data_root = match data_root {
        Some(root) => root.to_path_buf(),
        None => repomeow_data_root()?,
    };
    let wiki_directory = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_directory);
    let meta_path = wiki_directory.join(WIKI_META_FILE);
    let raw = fs::read_to_string(&meta_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ToolFailure::new("wiki_not_generated", "该项目尚未生成 Wiki")
                .with_detail(meta_path.to_string_lossy())
        } else {
            ToolFailure::new("wiki_meta_read_failed", "读取 Wiki meta.json 失败")
                .with_detail(error.to_string())
        }
    })?;
    let meta: Value = serde_json::from_str(&raw).map_err(|error| {
        ToolFailure::new("wiki_meta_invalid", "Wiki meta.json 格式无效")
            .with_detail(error.to_string())
    })?;
    if meta.get("status").and_then(Value::as_str) != Some("completed") {
        return Err(
            ToolFailure::new("wiki_not_generated", "该项目的 Wiki 尚未生成完成")
                .with_detail(meta_path.to_string_lossy()),
        );
    }

    Ok(WikiDirectoryOutput {
        project_directory,
        wiki_directory: wiki_directory.to_string_lossy().into_owned(),
        meta_path: meta_path.to_string_lossy().into_owned(),
        meta,
    })
}

fn repomeow_data_root() -> Result<PathBuf, ToolFailure> {
    if let Some(path) = env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    home_dir()
        .map(|home| home.join(APP_DATA_DIR_NAME))
        .ok_or_else(|| ToolFailure::new("home_directory_unavailable", "无法确定当前用户主目录"))
}

// ── 共享辅助 ──────────────────────────────────────────────────────────

/// 实体参数:含 "::" 视为 entityId 精确匹配,否则视为实体名;空白输入为 None。
fn split_entity_token(value: &str) -> Option<(Option<String>, Option<String>)> {
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
fn truncate_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

fn data_root_or_default(data_root: Option<&Path>) -> Result<PathBuf, ToolFailure> {
    match data_root {
        Some(root) => Ok(root.to_path_buf()),
        None => repomeow_data_root(),
    }
}

fn open_db(data_root: &Path) -> Result<Db, ToolFailure> {
    Db::open(&data_root.join(PROJECTS_DB_FILE))
        .map_err(|error| ToolFailure::from_app("打开 RepoMeow 数据库失败", error))
}

/// 按登记目录(归一化后)定位未归档项目 id。
fn resolve_project_id(conn: &Connection, directory: &str) -> Result<Option<i64>, ToolFailure> {
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

fn require_project_id(conn: &Connection, directory: &str) -> Result<i64, ToolFailure> {
    resolve_project_id(conn, directory)?.ok_or_else(|| {
        ToolFailure::new("project_not_found", "该项目未在 RepoMeow 登记或已归档")
            .with_detail(clean_str(directory))
    })
}

// ── Wiki 查询 ─────────────────────────────────────────────────────────

fn load_project_wiki(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<(String, crate::commands::wiki::WikiData), ToolFailure> {
    let project_directory = clean_str(project_directory);
    if project_directory.trim().is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }
    let data_root = data_root_or_default(data_root)?;
    let data = load_wiki_at(&data_root, &project_directory).ok_or_else(|| {
        ToolFailure::new("wiki_not_generated", "该项目尚未生成 Wiki(或 Wiki 未生成完成)")
    })?;
    Ok((project_directory, data))
}

fn list_wiki_pages_impl(
    input: ProjectDirectoryInput,
    data_root: Option<&Path>,
) -> Result<WikiPagesOutput, ToolFailure> {
    let (project_directory, data) = load_project_wiki(&input.project_directory, data_root)?;
    let pages = data
        .meta
        .outline
        .iter()
        .map(|page| {
            json!({
                "id": page.id,
                "title": page.title,
                "description": page.description,
                "section": page.section,
                "relevantFiles": page.relevant_files,
            })
        })
        .collect();
    Ok(WikiPagesOutput {
        project_directory,
        stale: data.stale,
        generated_at: data.meta.generated_at,
        head_sha: data.meta.head_sha,
        generator: data.meta.generator,
        model: data.meta.model,
        pages,
    })
}

fn read_wiki_page_impl(
    input: ReadWikiPageInput,
    data_root: Option<&Path>,
) -> Result<WikiPageOutput, ToolFailure> {
    let (_directory, data) = load_project_wiki(&input.project_directory, data_root)?;
    let page_id = input.page_id.trim();
    let Some(page) = data.pages.iter().find(|page| page.id == page_id) else {
        return Err(ToolFailure::new(
            "wiki_page_not_found",
            format!("未找到页面 id「{page_id}」,可用 list_wiki_pages 查看页面清单"),
        ));
    };
    let (content, truncated) = truncate_text(&page.content, WIKI_PAGE_MAX_BYTES);
    Ok(WikiPageOutput {
        id: page.id.clone(),
        title: page.title.clone(),
        file: page.file.clone(),
        stale: data.stale,
        content,
        truncated,
    })
}

// ── 项目洞察 ──────────────────────────────────────────────────────────

fn read_project_file_impl(input: ReadProjectFileInput) -> Result<ProjectFileOutput, ToolFailure> {
    let root = clean_str(&input.project_directory);
    if root.is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }
    let rel_path = to_forward_slash_str(input.path.trim());
    if rel_path.is_empty() {
        return Err(ToolFailure::new("invalid_file_path", "文件路径不能为空"));
    }
    let offset_line = input.offset_line.unwrap_or(1).max(1);
    let max_lines = input
        .max_lines
        .unwrap_or(READ_FILE_DEFAULT_LINES)
        .clamp(1, READ_FILE_MAX_LINES) as usize;
    // read_file_preview 内部已做 canonicalize + 根目录前缀校验,拒绝越界与符号链接逃逸。
    let preview = read_file_preview(root, rel_path.clone())
        .map_err(|error| ToolFailure::from_app("读取文件失败", error))?;
    let Some(text) = preview.text else {
        return Err(ToolFailure::new(
            "binary_file",
            format!("「{rel_path}」是二进制或非 UTF-8 文件,无法按行读取"),
        ));
    };
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok(ProjectFileOutput {
            path: rel_path,
            total_lines: 0,
            start_line: 0,
            end_line: 0,
            content: String::new(),
            has_more: false,
            preview_truncated: preview.truncated,
        });
    }
    let start = ((offset_line as usize).saturating_sub(1)).min(total);
    if start >= total {
        return Err(ToolFailure::new(
            "offset_out_of_range",
            format!("文件共 {total} 行,offsetLine={offset_line} 超出范围"),
        ));
    }
    let end = (start + max_lines).min(total);
    let content = lines[start..end]
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{}: {}", start + index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ProjectFileOutput {
        path: rel_path,
        total_lines: total,
        start_line: start as u64 + 1,
        end_line: end as u64,
        content,
        has_more: end < total,
        preview_truncated: preview.truncated,
    })
}

fn list_reports_impl(
    input: ListReportsInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = match &input.project_directory {
        Some(directory) => Some(require_project_id(&conn, directory)?),
        None => None,
    };
    let limit = Some(input.limit.unwrap_or(10).clamp(1, 50) as usize);
    let items = list_report_history_impl(&conn, limit, Some(0), project_id)
        .map_err(|error| ToolFailure::from_app("查询报告历史失败", error))?;
    Ok(json!({ "reports": items }))
}

fn list_custom_commands_impl(
    input: ProjectDirectoryInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, &input.project_directory)?;
    let commands = script::list_commands(&conn, project_id)
        .map_err(|error| ToolFailure::from_app("查询自定义命令失败", error))?;
    Ok(json!({
        "projectId": project_id,
        "commands": commands,
    }))
}

// ── 报告生成 ──────────────────────────────────────────────────────────

async fn generate_report_impl(
    input: GenerateReportInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let period_type = match input.period_type.trim() {
        "daily" => "daily",
        "weekly" => "weekly",
        _ => {
            return Err(ToolFailure::new(
                "invalid_period_type",
                "periodType 必须是 daily 或 weekly",
            ))
        }
    };
    if input.project_directories.is_empty() {
        return Err(ToolFailure::new(
            "project_directories_required",
            "projectDirectories 至少需要一个项目目录",
        ));
    }
    let today = Local::now().date_naive();
    let default_from = if period_type == "weekly" {
        today - chrono::Duration::days(6)
    } else {
        today
    };
    let date_from = input
        .date_from
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_from.format("%Y-%m-%d").to_string());
    let date_to = input
        .date_to
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| today.format("%Y-%m-%d").to_string());
    let range_label = if date_from != date_to {
        format!("{date_from} ~ {date_to}")
    } else {
        date_from.clone()
    };
    let author_mode = match input.author_mode.as_deref() {
        Some("me") => "me",
        _ => "all",
    };
    let language = match input.language.as_deref() {
        Some("en-US") => "en-US",
        _ => "zh-CN",
    };

    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let mut project_ids: Vec<i64> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    {
        let conn = db.0.lock().unwrap();
        for directory in &input.project_directories {
            match resolve_project_id(&conn, directory)? {
                Some(id) if !project_ids.contains(&id) => project_ids.push(id),
                Some(_) => {}
                None => unknown.push(clean_str(directory)),
            }
        }
    }
    if !unknown.is_empty() {
        return Err(
            ToolFailure::new("project_not_found", "以下目录未在 RepoMeow 登记或已归档")
                .with_detail(unknown.join("; ")),
        );
    }

    let request = GenerateAndSaveReportRequest {
        run_id: format!("mcp-{}", crate::time_util::now_ts_nanos()),
        project_ids,
        date_from,
        date_to,
        range_label: range_label.clone(),
        author_mode: author_mode.to_string(),
        language: language.to_string(),
        period_type: period_type.to_string(),
    };
    let Some(report) = mcp_generate_and_save_report(&data_root, &db, &request)
        .await
        .map_err(|error| ToolFailure::from_app("生成报告失败", error))?
    else {
        return Ok(json!({
            "generated": false,
            "rangeLabel": range_label,
            "message": "所选时间范围内没有提交记录,未生成报告。",
        }));
    };
    let (result, result_truncated) = truncate_text(&report.result, REPORT_RESULT_MAX_BYTES);
    Ok(json!({
        "generated": true,
        "historyId": report.history_id,
        "rangeLabel": range_label,
        "result": result,
        "resultTruncated": result_truncated,
        "projects": report
            .commit_data
            .iter()
            .map(|project| json!({
                "name": project.project_name,
                "commits": project.commits.len(),
            }))
            .collect::<Vec<_>>(),
    }))
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE").map(PathBuf::from).or_else(|| {
        let drive = env::var_os("HOMEDRIVE")?;
        let path = env::var_os("HOMEPATH")?;
        Some(PathBuf::from(drive).join(path))
    })
}

#[cfg(not(windows))]
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "repomeow-mcp-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos(),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(root: &Path, args: &[&str]) {
        let root = root.to_string_lossy();
        run_git(&root, args).unwrap();
    }

    #[test]
    fn commit_paths_only_accept_repository_relative_paths() {
        assert!(normalize_commit_paths(Some(vec!["src/main.rs".into()])).is_ok());
        assert!(normalize_commit_paths(Some(vec!["../secret".into()])).is_err());
        assert!(normalize_commit_paths(Some(vec!["C:/secret".into()])).is_err());
        assert!(normalize_commit_paths(Some(Vec::new())).is_err());
    }

    #[test]
    fn tool_groups_are_opt_in_and_filter_visible_routes() {
        const ALL_ROUTES: &[&str] = &[
            "commit_code",
            "get_git_status",
            "get_wiki_directory",
            "list_wiki_pages",
            "read_wiki_page",
            "sem_find",
            "sem_context",
            "sem_relations",
            "sem_diff",
            "read_project_file",
            "list_reports",
            "list_custom_commands",
            "generate_report",
        ];
        let disabled = RepoMeowMcpServer::new(McpToolGroups::default());
        for route in ALL_ROUTES {
            assert!(
                !disabled.tool_router.has_route(route),
                "route should be off by default: {route}"
            );
        }

        let enabled = RepoMeowMcpServer::new(McpToolGroups {
            git_commit: true,
            wiki: true,
            sem: true,
            project: true,
            report: true,
        });
        for route in ALL_ROUTES {
            assert!(
                enabled.tool_router.has_route(route),
                "route should be on: {route}"
            );
        }

        // 单组开关互不影响
        let wiki_only = RepoMeowMcpServer::new(McpToolGroups {
            wiki: true,
            ..McpToolGroups::default()
        });
        assert!(wiki_only.tool_router.has_route("list_wiki_pages"));
        assert!(!wiki_only.tool_router.has_route("sem_find"));
        assert!(!wiki_only.tool_router.has_route("generate_report"));
    }

    #[test]
    fn tool_group_settings_accept_store_strings_and_json_booleans() {
        let settings = json!({
            "mcpGitCommitEnabled": "true",
            "mcpWikiEnabled": true,
            "mcpSemEnabled": "true",
            "mcpProjectEnabled": true,
            "mcpReportEnabled": "true",
        });
        assert!(setting_bool(&settings, GIT_COMMIT_ENABLED_KEY));
        assert!(setting_bool(&settings, WIKI_ENABLED_KEY));
        assert!(setting_bool(&settings, SEM_ENABLED_KEY));
        assert!(setting_bool(&settings, PROJECT_ENABLED_KEY));
        assert!(setting_bool(&settings, REPORT_ENABLED_KEY));
        assert!(!setting_bool(&settings, "missing"));
    }

    #[test]
    fn wiki_directory_returns_completed_meta() {
        let data_root = temp_dir("wiki-completed");
        let project = temp_dir("wiki-project");
        let project_path = project.to_string_lossy().into_owned();
        let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_path);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(WIKI_META_FILE),
            r#"{"status":"completed","version":1,"outline":[]}"#,
        )
        .unwrap();

        let output = get_wiki_directory_impl(
            GetWikiDirectoryInput {
                project_directory: project_path,
            },
            Some(&data_root),
        )
        .unwrap();
        assert_eq!(output.meta["status"], "completed");
        assert_eq!(PathBuf::from(output.meta_path), dir.join(WIKI_META_FILE));

        let _ = fs::remove_dir_all(data_root);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn wiki_directory_rejects_missing_or_incomplete_meta() {
        let data_root = temp_dir("wiki-missing");
        let project_path = "D:/projects/missing".to_string();
        let missing = get_wiki_directory_impl(
            GetWikiDirectoryInput {
                project_directory: project_path.clone(),
            },
            Some(&data_root),
        )
        .unwrap_err();
        assert_eq!(missing.code, "wiki_not_generated");

        let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_path);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(WIKI_META_FILE), r#"{"status":"generating"}"#).unwrap();
        let incomplete = get_wiki_directory_impl(
            GetWikiDirectoryInput {
                project_directory: project_path,
            },
            Some(&data_root),
        )
        .unwrap_err();
        assert_eq!(incomplete.code, "wiki_not_generated");

        let _ = fs::remove_dir_all(data_root);
    }

    #[test]
    fn commit_code_can_commit_selected_files() {
        let root = temp_dir("commit-selected");
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.email", "mcp@example.com"]);
        git(&root, &["config", "user.name", "RepoMeow MCP"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();

        let output = commit_code_impl(CommitCodeInput {
            directory: root.to_string_lossy().into_owned(),
            message: "test: 仅提交 a".into(),
            files: Some(vec!["a.txt".into()]),
        })
        .unwrap();

        assert_eq!(output.branch.as_deref(), Some("main"));
        assert_eq!(output.committed_files, vec!["a.txt"]);
        let status = git_output(&output.directory, &["status", "--porcelain"], "status").unwrap();
        assert!(status.contains("?? b.txt"));

        let _ = fs::remove_dir_all(root);
    }

    fn seed_wiki(data_root: &Path, project_path: &str) {
        let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), project_path);
        fs::create_dir_all(dir.join("pages")).unwrap();
        fs::write(
            dir.join(WIKI_META_FILE),
            r#"{"status":"completed","version":1,"generatedAt":"2026-09-01 10:00","outline":[{"id":"overview","file":"01-overview.md","title":"总览","description":"项目总览","relevantFiles":["src/main.ts"]}]}"#,
        )
        .unwrap();
        fs::write(dir.join("pages").join("01-overview.md"), "# 总览\n\n这是内容。\n").unwrap();
    }

    #[test]
    fn wiki_pages_list_and_read_page() {
        let data_root = temp_dir("wiki-pages");
        let project = temp_dir("wiki-pages-project");
        let project_path = project.to_string_lossy().into_owned();
        seed_wiki(&data_root, &project_path);

        let list = list_wiki_pages_impl(
            ProjectDirectoryInput {
                project_directory: project_path.clone(),
            },
            Some(&data_root),
        )
        .unwrap();
        assert_eq!(list.pages.len(), 1);
        assert_eq!(list.pages[0]["id"], json!("overview"));
        assert!(!list.stale);

        let page = read_wiki_page_impl(
            ReadWikiPageInput {
                project_directory: project_path.clone(),
                page_id: "overview".into(),
            },
            Some(&data_root),
        )
        .unwrap();
        assert!(page.content.contains("这是内容"));
        assert!(!page.truncated);

        let missing = read_wiki_page_impl(
            ReadWikiPageInput {
                project_directory: project_path,
                page_id: "nope".into(),
            },
            Some(&data_root),
        )
        .unwrap_err();
        assert_eq!(missing.code, "wiki_page_not_found");

        let _ = fs::remove_dir_all(data_root);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn read_project_file_windows_lines_and_bounds() {
        let project = temp_dir("read-file");
        fs::write(project.join("a.txt"), "l1\nl2\nl3\nl4\nl5\n").unwrap();
        let root = project.to_string_lossy().into_owned();

        let page = read_project_file_impl(ReadProjectFileInput {
            project_directory: root.clone(),
            path: "a.txt".into(),
            offset_line: Some(2),
            max_lines: Some(2),
        })
        .unwrap();
        assert_eq!(page.start_line, 2);
        assert_eq!(page.end_line, 3);
        assert!(page.has_more);
        assert_eq!(page.content, "2: l2\n3: l3");

        let tail = read_project_file_impl(ReadProjectFileInput {
            project_directory: root.clone(),
            path: "a.txt".into(),
            offset_line: Some(4),
            max_lines: None,
        })
        .unwrap();
        assert!(!tail.has_more);
        assert_eq!(tail.end_line, 5);

        let out_of_range = read_project_file_impl(ReadProjectFileInput {
            project_directory: root.clone(),
            path: "a.txt".into(),
            offset_line: Some(99),
            max_lines: None,
        })
        .unwrap_err();
        assert_eq!(out_of_range.code, "offset_out_of_range");

        fs::write(project.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let binary = read_project_file_impl(ReadProjectFileInput {
            project_directory: root.clone(),
            path: "bin.dat".into(),
            offset_line: None,
            max_lines: None,
        })
        .unwrap_err();
        assert_eq!(binary.code, "binary_file");

        let escape = read_project_file_impl(ReadProjectFileInput {
            project_directory: root,
            path: "../outside.txt".into(),
            offset_line: None,
            max_lines: None,
        })
        .unwrap_err();
        assert!(!escape.code.is_empty());

        let _ = fs::remove_dir_all(project);
    }

    fn seed_db(data_root: &Path) {
        let db = Db::open(&data_root.join(PROJECTS_DB_FILE)).unwrap();
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (path, name, created_at, updated_at) VALUES (?1, ?2, 0, 0)",
            params![clean_str("D:/projects/demo"), "demo"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO custom_commands (project_id, name, command) VALUES (1, 'dev', 'pnpm dev')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn project_scoped_tools_resolve_registered_projects() {
        let data_root = temp_dir("mcp-db");
        seed_db(&data_root);

        let commands = list_custom_commands_impl(
            ProjectDirectoryInput {
                project_directory: "D:/projects/demo".into(),
            },
            Some(&data_root),
        )
        .unwrap();
        assert_eq!(commands["commands"][0]["name"], json!("dev"));

        let unknown = list_custom_commands_impl(
            ProjectDirectoryInput {
                project_directory: "D:/projects/ghost".into(),
            },
            Some(&data_root),
        )
        .unwrap_err();
        assert_eq!(unknown.code, "project_not_found");

        let reports = list_reports_impl(
            ListReportsInput {
                project_directory: None,
                limit: None,
            },
            Some(&data_root),
        )
        .unwrap();
        assert!(reports["reports"].as_array().unwrap().is_empty());

        let _ = fs::remove_dir_all(data_root);
    }

    #[tokio::test]
    async fn generate_report_validates_before_touching_disk() {
        let bad_period = generate_report_impl(
            GenerateReportInput {
                project_directories: vec!["D:/projects/demo".into()],
                period_type: "monthly".into(),
                date_from: None,
                date_to: None,
                author_mode: None,
                language: None,
            },
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(bad_period.code, "invalid_period_type");

        let no_projects = generate_report_impl(
            GenerateReportInput {
                project_directories: Vec::new(),
                period_type: "daily".into(),
                date_from: None,
                date_to: None,
                author_mode: None,
                language: None,
            },
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(no_projects.code, "project_directories_required");
    }

    #[test]
    fn entity_token_split_and_truncation() {
        assert_eq!(
            split_entity_token("src/a.ts::function::run"),
            Some((Some("src/a.ts::function::run".to_string()), None))
        );
        assert_eq!(
            split_entity_token("run"),
            Some((None, Some("run".to_string())))
        );
        assert_eq!(split_entity_token("  "), None);

        let (text, truncated) = truncate_text("hello", 10);
        assert!(!truncated);
        assert_eq!(text, "hello");
        let (text, truncated) = truncate_text("你好世界", 7);
        assert!(truncated);
        assert_eq!(text, "你好");
    }
}
