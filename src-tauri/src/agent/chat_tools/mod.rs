//! 项目问答(chat)的 RepoMeow 工具集。
//!
//! 把既有域命令(语义分析 / wiki / 自定义命令 / 报告 / 文件读取 / AI 配置)
//! 包装成 pi `AgentTool`,供 `commands/chat.rs` 构建的 Agent 使用。工具结果统一为
//! 面向 LLM 的文本,超长按 UTF-8 边界截断并标注;执行失败返回 `Err`,
//! 由 agent-loop 转成 error 工具结果回传模型。
//!
//! 注意:本模块是新文件,需在 `agent/mod.rs` 挂 `pub mod chat_tools;`
//! (对齐阶段由主智能体处理)。

use futures::future::BoxFuture;
use tauri::AppHandle;

use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionError};

mod cmds;
mod config;
mod file;
mod report;
mod sem;
#[cfg(test)]
mod tests;
mod util;
mod wiki;

use cmds::*;
use config::*;
use file::*;
use report::*;
use sem::*;
use util::*;
use wiki::*;


/// 普通工具结果的字节上限。
const TOOL_RESULT_MAX_BYTES: usize = 16 * 1024;
/// read_wiki 单页正文的字节上限。
const WIKI_PAGE_MAX_BYTES: usize = 24 * 1024;
/// generate_report 返回正文的字节上限。
const REPORT_RESULT_MAX_BYTES: usize = 4 * 1024;
/// read_project_file 默认/最大返回行数。
const READ_FILE_DEFAULT_LINES: u64 = 400;
const READ_FILE_MAX_LINES: u64 = 5000;

type ToolFuture = BoxFuture<'static, Result<AgentToolResult, ToolExecutionError>>;

/// 项目问答工具的共享上下文。代码级操作的实际工作目录是
/// `worktree_path.unwrap_or(project_path)`;wiki 操作始终使用登记的
/// `project_path`(wiki 目录按项目路径派生)。
#[derive(Debug, Clone, Default)]
pub struct ChatToolContext {
    pub project_path: String,
    pub project_name: String,
    /// projects 表主键;项目未登记时为 None(登记类工具会报错)。
    pub project_id: Option<i64>,
    pub worktree_path: Option<String>,
}

impl ChatToolContext {
    fn work_dir(&self) -> String {
        self.worktree_path
            .clone()
            .unwrap_or_else(|| self.project_path.clone())
    }
}

/// 构建项目问答工具集(每个工具的 description 面向 LLM,写明何时用与参数含义)。
pub fn chat_tools(app: AppHandle, ctx: ChatToolContext) -> Vec<AgentTool> {
    vec![
        sem_find_tool(&app, &ctx),
        sem_context_tool(&app, &ctx),
        sem_relations_tool(&app, &ctx),
        sem_diff_tool(&app, &ctx),
        read_wiki_tool(&app, &ctx),
        update_wiki_tool(&app, &ctx),
        regenerate_wiki_tool(&app, &ctx),
        list_custom_commands_tool(&app, &ctx),
        add_custom_command_tool(&app, &ctx),
        generate_report_tool(&app, &ctx),
        list_reports_tool(&app, &ctx),
        read_project_file_tool(&ctx),
        get_ai_config_tool(&app, &ctx),
        set_wiki_model_tool(&app, &ctx),
    ]
}

