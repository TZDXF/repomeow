use std::env;
use std::fs;
use std::path::Path;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, transport::stdio, ServerHandler, ServiceExt};
use serde_json::{json, Value};

use crate::commands::semantic;
use crate::path_util::clean_str;

mod git_tool;
mod project_tool;
mod report_tool;
mod types;
mod util;
mod wiki_tool;
#[cfg(test)]
mod tests;

use git_tool::*;
use project_tool::*;
use report_tool::*;
use types::*;
use util::*;
use wiki_tool::*;

const SETTINGS_FILE: &str = "settings.json";
const GIT_COMMIT_ENABLED_KEY: &str = "mcpGitCommitEnabled";
const WIKI_ENABLED_KEY: &str = "mcpWikiEnabled";
const SEM_ENABLED_KEY: &str = "mcpSemEnabled";
const PROJECT_ENABLED_KEY: &str = "mcpProjectEnabled";
const REPORT_ENABLED_KEY: &str = "mcpReportEnabled";

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

