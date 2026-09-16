//! RepoMeow CLI:主程序内置的命令行模式,面向 AI agent 与脚本提供
//! Git、Wiki、语义分析、项目数据与报告能力。
//!
//! - 首参数命中子命令(git/wiki/sem/project/report)即进入 CLI 模式,执行后退出,不启动桌面窗口;
//! - 成功结果以 JSON 写 stdout,失败以 {"code","message","detail"} JSON 写 stderr 且退出码非 0;
//! - 数据目录默认 `~/.repomeow`,可用环境变量 REPOMEOW_DATA_DIR 覆盖。

use std::env;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::commands::semantic;
use crate::path_util::clean_str;

mod git_tool;
mod project_tool;
mod report_tool;
pub(crate) mod skills;
#[cfg(test)]
mod tests;
mod types;
mod util;
mod wiki_tool;

use git_tool::*;
use project_tool::*;
use report_tool::*;
use types::*;
use util::*;
use wiki_tool::*;

#[derive(Debug, Parser)]
#[command(
    name = "repomeow",
    version,
    about = "RepoMeow CLI:Git / Wiki / 语义分析 / 项目数据 / 报告(输出均为 JSON)",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Git 仓库状态与提交
    #[command(subcommand)]
    Git(GitCommands),
    /// 项目 Wiki 查询
    #[command(subcommand)]
    Wiki(WikiCommands),
    /// 代码语义分析
    #[command(subcommand)]
    Sem(SemCommands),
    /// 项目数据查询(只读)
    #[command(subcommand)]
    Project(ProjectCommands),
    /// 日报/周报
    #[command(subcommand)]
    Report(ReportCommands),
}

#[derive(Debug, Subcommand)]
enum GitCommands {
    /// 状态摘要:分支、与上游领先/落后、变更计数、最后抓取与提交时间
    Status {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
    },
    /// 创建提交;--files 省略时提交全部变更(含未跟踪文件)
    Commit {
        /// Git 仓库目录
        #[arg(short, long)]
        directory: String,
        /// 提交信息,不能为空
        #[arg(short, long)]
        message: String,
        /// 仅提交指定的仓库相对路径(逗号分隔或重复传入);拒绝绝对路径与 ..
        #[arg(long, value_delimiter = ',')]
        files: Option<Vec<String>>,
    },
}

#[derive(Debug, Subcommand)]
enum WikiCommands {
    /// 获取已生成 Wiki 的目录与 meta.json 元数据
    Dir {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 列出 Wiki 大纲(每页 id/标题/简介/分区/来源文件,及 stale 标记)
    Pages {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 读取 Wiki 某一页正文(页面 id 来自 pages;超长截断)
    Read {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 页面 id(来自 pages 的大纲清单)
        #[arg(long)]
        page_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum SemCommands {
    /// 按名称语义搜索代码实体(函数/类/接口/结构体等)
    Find {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
        /// 搜索关键词:实体名或其一部分
        #[arg(short, long)]
        query: String,
    },
    /// 查看实体语义上下文:源码摘要 + 按调用/引用关系扩展的相关实体
    Context {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
        /// 实体名或 entityId(形如 src/a.ts::function::run)
        #[arg(short, long)]
        entity: String,
        /// 实体所在文件的仓库相对路径(/ 分隔),重名时消歧
        #[arg(long)]
        file_path: Option<String>,
        /// 上下文预算(token 数,500-4000),缺省 2000
        #[arg(long)]
        budget: Option<u32>,
        /// 关系扩展跳数(0-3),缺省 1
        #[arg(long)]
        hops: Option<u32>,
    },
    /// 查询实体的直接调用方(callers)与引用点(refs)
    Relations {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
        /// 实体名或 entityId(含 "::" 视为 entityId)
        #[arg(short, long)]
        entity: String,
        /// 实体所在文件的仓库相对路径(/ 分隔),重名时消歧
        #[arg(long)]
        file_path: Option<String>,
    },
    /// 汇总仓库当前未提交的实体级结构化变更
    Diff {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommands {
    /// 读取项目内单个文本文件的指定行区间(content 带 1-based 行号前缀)
    ReadFile {
        /// 项目目录(读取范围以该目录为根,拒绝越界与符号链接逃逸)
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 项目内相对路径(/ 分隔)
        #[arg(long)]
        path: String,
        /// 起始行(1-based),默认 1
        #[arg(long)]
        offset_line: Option<u64>,
        /// 最多返回行数,默认 400,上限 5000
        #[arg(long)]
        max_lines: Option<u64>,
    },
    /// 列出已生成的日报/周报历史(按生成时间倒序)
    Reports {
        /// 限定单个项目(RepoMeow 登记目录);缺省列出全部项目
        #[arg(short = 'p', long)]
        project_directory: Option<String>,
        /// 返回条数(1-50),默认 10
        #[arg(long)]
        limit: Option<u32>,
    },
    /// 列出项目登记的自定义命令(了解项目构建/运行方式的捷径)
    Commands {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
}

#[derive(Debug, Subcommand)]
enum ReportCommands {
    /// 为一个或多个已登记项目生成日报/周报(调用 AI 并写入报告历史,消耗 AI 额度)
    Generate {
        /// 项目目录列表(逗号分隔或重复传入;必须已登记且未归档)
        #[arg(short = 'p', long, value_delimiter = ',', required = true)]
        project_directories: Vec<String>,
        /// 报告类型:daily 日报 / weekly 周报
        #[arg(long)]
        period_type: String,
        /// 起始日期 YYYY-MM-DD;缺省 daily=今天、weekly=6 天前
        #[arg(long)]
        date_from: Option<String>,
        /// 结束日期 YYYY-MM-DD;缺省今天
        #[arg(long)]
        date_to: Option<String>,
        /// 提交作者范围:all 全部(默认)/ me 仅当前 git 用户
        #[arg(long)]
        author_mode: Option<String>,
        /// 报告语言:zh-CN(默认)/ en-US
        #[arg(long)]
        language: Option<String>,
    },
}

/// CLI 首参数白名单:命中即进入 CLI 模式。
fn is_cli_head(arg: &str) -> bool {
    matches!(
        arg,
        "git"
            | "wiki"
            | "sem"
            | "project"
            | "report"
            | "help"
            | "-h"
            | "--help"
            | "-V"
            | "--version"
    )
}

/// 判断是否应以 CLI 模式运行(在 Tauri 初始化前调用)。
pub fn is_cli_mode() -> bool {
    env::args_os()
        .nth(1)
        .and_then(|arg| arg.into_string().ok())
        .is_some_and(|arg| is_cli_head(&arg))
}

/// 以 CLI 模式执行并返回进程退出码。
pub fn run_cli_blocking() -> i32 {
    #[cfg(windows)]
    attach_parent_console();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "{}",
                json!({"code": "runtime_init_failed", "message": "初始化 CLI 运行时失败", "detail": error.to_string()})
            );
            return 1;
        }
    };
    runtime.block_on(run_cli())
}

/// release 为 windows_subsystem="windows" 的 GUI 进程,默认无控制台;
/// 附加到父进程控制台使终端内可见输出(输出被管道重定向时本就可写,不受影响)。
#[cfg(windows)]
fn attach_parent_console() {
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

/// 帮助与版本保留文本输出;参数解析失败遵循 CLI 的 JSON 错误协议。
fn format_parse_error(error: clap::Error) -> (i32, String) {
    let exit_code = error.exit_code();
    let output = if exit_code == 0 {
        error.to_string()
    } else {
        ToolFailure::new("invalid_arguments", "CLI 参数无效")
            .with_detail(error.to_string())
            .to_json()
            .to_string()
    };
    (exit_code, output)
}

async fn run_cli() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let (exit_code, output) = format_parse_error(error);
            if exit_code == 0 {
                print!("{output}");
            } else {
                eprintln!("{output}");
            }
            return exit_code;
        }
    };
    match dispatch(cli.command).await {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(failure) => {
            eprintln!("{}", failure.to_json());
            1
        }
    }
}

async fn dispatch(command: Commands) -> Result<Value, ToolFailure> {
    match command {
        Commands::Git(GitCommands::Status { directory }) => {
            let directory = clean_str(&directory);
            let status =
                tokio::task::spawn_blocking(move || crate::commands::git::status(&directory))
                    .await
                    .map_err(|error| {
                        ToolFailure::new("git_task_failed", "Git 状态任务执行失败")
                            .with_detail(error.to_string())
                    })?
                    .map_err(|error| ToolFailure::from_app("读取 Git 状态失败", error))?;
            Ok(json!(status))
        }
        Commands::Git(GitCommands::Commit {
            directory,
            message,
            files,
        }) => {
            let input = CommitCodeInput {
                directory,
                message,
                files,
            };
            let output = tokio::task::spawn_blocking(move || commit_code_impl(input))
                .await
                .map_err(|error| {
                    ToolFailure::new("git_task_failed", "代码提交任务执行失败")
                        .with_detail(error.to_string())
                })??;
            Ok(json!(output))
        }
        Commands::Wiki(WikiCommands::Dir { project_directory }) => Ok(json!(
            get_wiki_directory_impl(GetWikiDirectoryInput { project_directory }, None)?
        )),
        Commands::Wiki(WikiCommands::Pages { project_directory }) => Ok(json!(
            list_wiki_pages_impl(ProjectDirectoryInput { project_directory }, None)?
        )),
        Commands::Wiki(WikiCommands::Read {
            project_directory,
            page_id,
        }) => Ok(json!(read_wiki_page_impl(
            ReadWikiPageInput {
                project_directory,
                page_id,
            },
            None
        )?)),
        Commands::Sem(SemCommands::Find { directory, query }) => {
            let query = query.trim().to_string();
            if query.is_empty() {
                return Err(ToolFailure::new("invalid_query", "搜索关键词不能为空"));
            }
            let result = semantic::cli_semantic_find_entities(clean_str(&directory), query)
                .await
                .map_err(|error| ToolFailure::from_app("语义搜索失败", error))?;
            Ok(json!(result))
        }
        Commands::Sem(SemCommands::Context {
            directory,
            entity,
            file_path,
            budget,
            hops,
        }) => {
            let Some((entity_id, entity_name)) = split_entity_token(&entity) else {
                return Err(ToolFailure::new("invalid_entity", "实体名不能为空"));
            };
            let result = semantic::cli_semantic_entity_context(
                clean_str(&directory),
                entity_id,
                entity_name,
                file_path,
                budget.map(|value| value as usize),
                hops.map(|value| value as usize),
            )
            .await
            .map_err(|error| ToolFailure::from_app("读取实体上下文失败", error))?;
            Ok(json!(result))
        }
        Commands::Sem(SemCommands::Relations {
            directory,
            entity,
            file_path,
        }) => {
            let Some((entity_id, entity_name)) = split_entity_token(&entity) else {
                return Err(ToolFailure::new("invalid_entity", "实体名不能为空"));
            };
            let (callers, refs) = semantic::cli_semantic_entity_relations(
                clean_str(&directory),
                entity_id,
                entity_name,
                file_path,
            )
            .await
            .map_err(|error| ToolFailure::from_app("查询实体关系失败", error))?;
            Ok(json!({ "callers": callers, "refs": refs }))
        }
        Commands::Sem(SemCommands::Diff { directory }) => {
            let result = semantic::cli_semantic_worktree_diff(clean_str(&directory))
                .await
                .map_err(|error| ToolFailure::from_app("读取未提交变更失败", error))?;
            Ok(json!(result))
        }
        Commands::Project(ProjectCommands::ReadFile {
            project_directory,
            path,
            offset_line,
            max_lines,
        }) => Ok(json!(read_project_file_impl(ReadProjectFileInput {
            project_directory,
            path,
            offset_line,
            max_lines,
        })?)),
        Commands::Project(ProjectCommands::Reports {
            project_directory,
            limit,
        }) => list_reports_impl(
            ListReportsInput {
                project_directory,
                limit,
            },
            None,
        ),
        Commands::Project(ProjectCommands::Commands { project_directory }) => {
            list_custom_commands_impl(ProjectDirectoryInput { project_directory }, None)
        }
        Commands::Report(ReportCommands::Generate {
            project_directories,
            period_type,
            date_from,
            date_to,
            author_mode,
            language,
        }) => {
            generate_report_impl(
                GenerateReportInput {
                    project_directories,
                    period_type,
                    date_from,
                    date_to,
                    author_mode,
                    language,
                },
                None,
            )
            .await
        }
    }
}
