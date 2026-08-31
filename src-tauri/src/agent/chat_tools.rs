//! 项目问答(chat)的 RepoMeow 工具集。
//!
//! 把既有域命令(语义分析 / wiki / 自定义命令 / 报告 / 文件读取)包装成
//! pi `AgentTool`,供 `commands/chat.rs` 构建的 Agent 使用。工具结果统一为
//! 面向 LLM 的文本,超长按 UTF-8 边界截断并标注;执行失败返回 `Err`,
//! 由 agent-loop 转成 error 工具结果回传模型。
//!
//! 注意:本模块是新文件,需在 `agent/mod.rs` 挂 `pub mod chat_tools;`
//! (对齐阶段由主智能体处理)。

use std::sync::Arc;

use chrono::{Local, TimeZone};
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

use crate::agent::types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, ToolExecutionError, ToolExecutionMode,
};
use crate::commands::ai::{
    ai_generate_and_save_report, ai_generate_wiki, ai_update_wiki, GenerateAndSaveReportRequest,
    GenerateWikiRequest, UpdateWikiRequest, WikiGenerationEvent, WikiUpdateEvent,
};
use crate::commands::files::read_file_preview;
use crate::commands::report;
use crate::commands::script;
use crate::commands::semantic::{
    semantic_entity_callers, semantic_entity_context, semantic_entity_refs, semantic_find_entities,
    semantic_worktree_diff,
};
use crate::commands::wiki::{load_wiki, WikiOutlinePage};
use crate::error::{AppError, ErrorCode};
use crate::path_util::to_forward_slash_str;
use crate::time_util::now_ts_nanos;
use crate::db::Db;

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
    ]
}

// ── 组装辅助 ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tool(
    name: &str,
    label: &str,
    description: &str,
    parameters: Value,
    sequential: bool,
    execute: impl Fn(Value, Option<AgentToolUpdateCallback>) -> ToolFuture + Send + Sync + 'static,
) -> AgentTool {
    AgentTool {
        name: name.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        parameters,
        execution_mode: sequential.then_some(ToolExecutionMode::Sequential),
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, args, _signal, on_update| {
            execute(args, on_update)
        }),
    }
}

fn tool_err(error: AppError) -> ToolExecutionError {
    Box::new(error)
}

fn text_result(text: impl Into<String>) -> Result<AgentToolResult, ToolExecutionError> {
    Ok(AgentToolResult::text(text))
}

fn invalid_arg(name: &str) -> AppError {
    AppError::coded(
        ErrorCode::AiRequestFailed,
        format!("invalid or missing argument: {name}"),
    )
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn require_str(args: &Value, key: &str) -> Result<String, ToolExecutionError> {
    arg_str(args, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| tool_err(invalid_arg(key)))
}

fn arg_u64_opt(args: &Value, key: &str) -> Result<Option<u64>, ToolExecutionError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| tool_err(invalid_arg(key))),
    }
}

/// 实体参数:含 "::" 视为 entityId 精确匹配,否则视为实体名。
fn split_entity_token(value: &str) -> (Option<String>, Option<String>) {
    if value.contains("::") {
        (Some(value.to_string()), None)
    } else {
        (None, Some(value.to_string()))
    }
}

/// 按 UTF-8 边界截断并标注原始长度。
fn truncate_bytes(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n…(已截断,原文 {} 字节)",
        &text[..end],
        text.len()
    )
}

fn pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
}

/// Rust 侧自生成的关联标识(8-4-4-4-12 的 UUID v4 形状;熵来自 RandomState,
/// 非密码学随机,仅用于 sem/报告请求的 requestId 关联与取消)。
fn pseudo_request_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u64(now_ts_nanos() as u64);
    first.write_u64(u64::from(std::process::id()));
    let hi = first.finish();
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u64(hi);
    second.write_u64(now_ts_nanos() as u64);
    let lo = second.finish();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:01x}{:03x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xffff,
        hi & 0x0fff,
        8 | ((lo >> 62) & 0x3),
        (lo >> 48) & 0x0fff,
        lo & 0xffff_ffff_ffff,
    )
}

// ── 语义分析 ─────────────────────────────────────────────────────────

fn sem_find_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "sem_find",
        "语义搜索",
        "按名称语义搜索项目内的代码实体(函数/类/接口/结构体等)。回答「XX 在哪里实现」「项目里有没有 XX」前先用它定位,再用 sem_context 查看上下文。参数:query(必填)——搜索关键词,实体名或其一部分。返回 JSON:命中实体列表(entityId/name/entityType/filePath/startLine/endLine)与 truncated 标记。",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "搜索关键词:实体名或其一部分,如 \"debounce\"、\"WikiGenKernel\"。"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let work_dir = ctx.work_dir();
            move |args, _on_update| {
                let app = app.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    let query = require_str(&args, "query")?;
                    let result = semantic_find_entities(app, work_dir, query, Some(pseudo_request_id()))
                        .await
                        .map_err(tool_err)?;
                    text_result(truncate_bytes(pretty_json(&result), TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}

fn sem_context_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "sem_context",
        "实体上下文",
        "查看某个代码实体的语义上下文:实体源码摘要与按调用/引用关系扩展出的相关实体。在 sem_find 定位到实体后,用它理解实现细节。参数:entity(必填)——实体名或 entityId(形如 src/a.ts::function::run,含 \"::\" 时按 entityId 精确匹配);file_path(可选)——实体所在文件的仓库相对路径,重名时用于消歧;budget(可选)——上下文预算(token 数);hops(可选)——关系扩展跳数。",
        json!({
            "type": "object",
            "properties": {
                "entity": {
                    "type": "string",
                    "description": "实体名或 entityId(含 \"::\" 的串视为 entityId)。"
                },
                "file_path": {
                    "type": "string",
                    "description": "实体所在文件的仓库相对路径(/ 分隔),可选。"
                },
                "budget": {
                    "type": "integer",
                    "description": "上下文预算(token 数),可选。"
                },
                "hops": {
                    "type": "integer",
                    "description": "关系扩展跳数,可选。"
                }
            },
            "required": ["entity"],
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let work_dir = ctx.work_dir();
            move |args, _on_update| {
                let app = app.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    let entity = require_str(&args, "entity")?;
                    let (entity_id, entity_name) = split_entity_token(&entity);
                    let file_path = arg_str(&args, "file_path").map(str::to_string);
                    let budget = arg_u64_opt(&args, "budget")?.map(|value| value as usize);
                    let hops = arg_u64_opt(&args, "hops")?.map(|value| value as usize);
                    let result = semantic_entity_context(
                        app,
                        work_dir,
                        entity_id,
                        entity_name,
                        file_path,
                        budget,
                        hops,
                        Some(pseudo_request_id()),
                    )
                    .await
                    .map_err(tool_err)?;
                    text_result(truncate_bytes(pretty_json(&result), TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}

fn sem_relations_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "sem_relations",
        "调用与引用",
        "查询某实体的直接调用方(callers)与引用点(refs),两次查询合并返回。用于回答「谁调用了 XX」「改动 XX 会影响哪些地方」。参数:entity(必填)——同 sem_context;file_path(可选)——重名消歧。",
        json!({
            "type": "object",
            "properties": {
                "entity": {
                    "type": "string",
                    "description": "实体名或 entityId(含 \"::\" 的串视为 entityId)。"
                },
                "file_path": {
                    "type": "string",
                    "description": "实体所在文件的仓库相对路径(/ 分隔),可选。"
                }
            },
            "required": ["entity"],
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let work_dir = ctx.work_dir();
            move |args, _on_update| {
                let app = app.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    let entity = require_str(&args, "entity")?;
                    let (entity_id, entity_name) = split_entity_token(&entity);
                    let file_path = arg_str(&args, "file_path").map(str::to_string);
                    let callers = semantic_entity_callers(
                        app.clone(),
                        work_dir.clone(),
                        entity_id.clone(),
                        entity_name.clone(),
                        file_path.clone(),
                        Some(pseudo_request_id()),
                    )
                    .await
                    .map_err(tool_err)?;
                    let refs = semantic_entity_refs(
                        app,
                        work_dir,
                        entity_id,
                        entity_name,
                        file_path,
                        Some(pseudo_request_id()),
                    )
                    .await
                    .map_err(tool_err)?;
                    let merged = json!({
                        "callers": serde_json::to_value(&callers).unwrap_or(Value::Null),
                        "refs": serde_json::to_value(&refs).unwrap_or(Value::Null),
                    });
                    text_result(truncate_bytes(pretty_json(&merged), TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}

fn sem_diff_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "sem_diff",
        "未提交变更",
        "汇总当前项目未提交的代码变更(实体级结构化差异摘要)。回答「当前改了什么」「最近在做什么」前先调用;生成日报/周报内容也应以它为参考之一。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let work_dir = ctx.work_dir();
            move |_args, _on_update| {
                let app = app.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    let result =
                        semantic_worktree_diff(app, work_dir, Some(pseudo_request_id()))
                            .await
                            .map_err(tool_err)?;
                    text_result(truncate_bytes(pretty_json(&result), TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}

// ── Wiki ─────────────────────────────────────────────────────────────

fn page_list(pages: &[WikiOutlinePage]) -> String {
    pages
        .iter()
        .map(|page| {
            let section = page
                .section
                .as_deref()
                .map(|value| format!("({value})"))
                .unwrap_or_default();
            format!(
                "- [{}] {}{}—— {}(来源文件:{})",
                page.id,
                page.title,
                section,
                page.description,
                if page.relevant_files.is_empty() {
                    "无".to_string()
                } else {
                    page.relevant_files.join(", ")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_wiki_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "read_wiki",
        "读 Wiki",
        "读取本项目由 RepoMeow 生成的 Wiki 文档。不传参数时返回 Wiki 大纲(页面清单:id/标题/简介/来源文件)与是否过时;传入 page_id 时返回该页正文 Markdown(超长会截断)。回答「这个项目是做什么的」「XX 模块怎么设计」时优先查 Wiki。参数:page_id(可选)——大纲中的页面 id。",
        json!({
            "type": "object",
            "properties": {
                "page_id": {
                    "type": "string",
                    "description": "要读取的页面 id(来自大纲清单),缺省返回大纲。"
                }
            },
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_path = ctx.project_path.clone();
            move |args, _on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let data = load_wiki(app, project_path)
                        .map_err(tool_err)?;
                    let Some(data) = data else {
                        return text_result(
                            "该项目尚未生成 Wiki。可调用 regenerate_wiki 生成整本(后台执行,耗时较长)。",
                        );
                    };
                    match arg_str(&args, "page_id") {
                        Some(page_id) => match data.pages.iter().find(|page| page.id == page_id) {
                            Some(page) => text_result(truncate_bytes(
                                format!("## {}\n\n{}", page.title, page.content),
                                WIKI_PAGE_MAX_BYTES,
                            )),
                            None => text_result(format!(
                                "未找到页面 id「{page_id}」。当前页面清单:\n{}",
                                page_list(&data.meta.outline)
                            )),
                        },
                        None => {
                            let stale_note = if data.stale {
                                "是(代码已更新,Wiki 可能过时,可调用 update_wiki 增量更新)"
                            } else {
                                "否"
                            };
                            text_result(format!(
                                "Wiki 概览(共 {} 页;过时:{stale_note}):\n{}",
                                data.meta.outline.len(),
                                page_list(&data.meta.outline)
                            ))
                        }
                    }
                })
            }
        },
    )
}

fn update_wiki_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "update_wiki",
        "增量更新 Wiki",
        "对已有 Wiki 做增量更新:检测自上次生成以来的代码变更,仅重新生成受影响页面(同步等待完成,通常几秒到一两分钟)。Wiki 过时(read_wiki 返回「过时:是」)时调用。项目还没有 Wiki 或历史被改写时本工具会报错,此时改用 regenerate_wiki。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_path = ctx.project_path.clone();
            move |_args, on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    if let Some(on_update) = &on_update {
                        on_update(AgentToolResult::text(
                            "正在增量更新 Wiki(检测变更并重生成受影响页面)…",
                        ));
                    }
                    // UpdateWikiRequest / WikiUpdateResult 的字段当前是 pub(super),
                    // 对齐阶段需放宽为 pub(crate)(见对齐清单)。
                    let request = UpdateWikiRequest {
                        run_id: pseudo_request_id(),
                        project_path: project_path.clone(),
                        language: "zh-CN".to_string(),
                        automatic: false,
                    };
                    let channel: Channel<WikiUpdateEvent> = Channel::new(|_| Ok(()));
                    let db = app.state::<Db>();
                    let result = ai_update_wiki(app.clone(), db, request, channel)
                        .await
                        .map_err(tool_err)?;
                    if result.updated_page_ids.is_empty() {
                        text_result("Wiki 已是最新,无需更新。")
                    } else {
                        text_result(format!(
                            "增量更新完成,已重新生成 {} 个页面:{}",
                            result.updated_page_ids.len(),
                            result.updated_page_ids.join(", ")
                        ))
                    }
                })
            }
        },
    )
}

fn regenerate_wiki_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "regenerate_wiki",
        "整本重生成 Wiki",
        "在后台启动整本 Wiki 重新生成(立即返回,不等待完成;耗时几分钟到几十分钟)。用于首次生成,或 Wiki 结构过时/用户明确要求重写整本时。不要为了更新少数几页而使用它——那是 update_wiki 的职责。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_path = ctx.project_path.clone();
            let project_name = ctx.project_name.clone();
            move |_args, _on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                let project_name = project_name.clone();
                Box::pin(async move {
                    // GenerateWikiRequest 的字段当前是 pub(super),
                    // 对齐阶段需放宽为 pub(crate)(见对齐清单)。
                    let request = GenerateWikiRequest {
                        run_id: pseudo_request_id(),
                        project_path: project_path.clone(),
                        project_name: project_name.clone(),
                        language: "zh-CN".to_string(),
                        concurrency: 2,
                    };
                    let channel: Channel<WikiGenerationEvent> = Channel::new(|_| Ok(()));
                    let task_app = app.clone();
                    tokio::spawn(async move {
                        let db = task_app.state::<Db>();
                        if let Err(error) = ai_generate_wiki(task_app.clone(), db, request, channel).await {
                            eprintln!("[chat] regenerate_wiki 后台任务失败: {error}");
                        }
                    });
                    text_result(
                        "已在后台启动整本 Wiki 重生成,完成后可在项目的 Wiki 面板查看;期间可以继续提问。",
                    )
                })
            }
        },
    )
}

// ── 自定义命令 ───────────────────────────────────────────────────────

fn list_custom_commands_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "list_custom_commands",
        "自定义命令清单",
        "列出当前项目已登记的自定义命令(名称/命令文本/描述)。用户问「有哪些自定义命令」「怎么跑 XX」时使用;需要新增命令时配合 add_custom_command。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |_args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let Some(project_id) = project_id else {
                        return text_result(
                            "当前项目未在 RepoMeow 登记(无 project_id),无法管理自定义命令。",
                        );
                    };
                    let db = app.state::<Db>();
                    let commands = {
                        let conn = db.0.lock().unwrap();
                        script::list_commands(&conn, project_id).map_err(tool_err)?
                    };
                    if commands.is_empty() {
                        return text_result("该项目暂无自定义命令。");
                    }
                    text_result(
                        commands
                            .iter()
                            .map(|command| {
                                if command.description.is_empty() {
                                    format!("- {}:`{}`", command.name, command.command)
                                } else {
                                    format!(
                                        "- {}:`{}`({})",
                                        command.name, command.command, command.description
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
            }
        },
    )
}

fn add_custom_command_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "add_custom_command",
        "新增自定义命令",
        "为当前项目新增一条自定义命令(保存到 RepoMeow,用户可在界面一键在终端执行)。仅在用户明确要求「添加/保存命令」时使用;执行前必须在回答中说明将写入的内容。参数:name(必填)命令名称;command(必填)将在终端执行的命令文本;description(可选)用途说明。",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "命令名称(项目内唯一)。"
                },
                "command": {
                    "type": "string",
                    "description": "将在终端执行的命令文本。"
                },
                "description": {
                    "type": "string",
                    "description": "用途说明,可选。"
                }
            },
            "required": ["name", "command"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            let project_path = ctx.project_path.clone();
            move |args, _on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let name = require_str(&args, "name")?;
                    let command = require_str(&args, "command")?;
                    let description =
                        arg_str(&args, "description").unwrap_or_default().trim().to_string();
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            project_path,
                        )));
                    };
                    let db = app.state::<Db>();
                    let created = {
                        let conn = db.0.lock().unwrap();
                        script::create_command(&conn, project_id, &name, &command, &description, "")
                            .map_err(tool_err)?
                    };
                    // 通知详情页 CustomCommands 卡片刷新,新建命令立即可见
                    script::emit_custom_commands_changed(&app);
                    text_result(format!(
                        "已创建自定义命令「{}」:`{}`",
                        created.name, created.command
                    ))
                })
            }
        },
    )
}

// ── 报告 ─────────────────────────────────────────────────────────────

fn generate_report_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "generate_report",
        "生成日报/周报",
        "生成项目日报或周报:汇总指定时间范围的 git 提交,调用 AI 生成正文并保存到报告历史(同步等待,通常十几秒)。用户要求「日报/周报/总结这段时间的工作」时使用。参数:period_type(必填)\"daily\" 或 \"weekly\";date_from/date_to(可选)\"YYYY-MM-DD\",缺省 daily=今天、weekly=最近 7 天;author_mode(可选)\"all\" 统计所有人、\"me\" 仅当前 git 用户,缺省 \"all\"。返回报告正文(超长截断)。",
        json!({
            "type": "object",
            "properties": {
                "period_type": {
                    "type": "string",
                    "enum": ["daily", "weekly"],
                    "description": "报告类型:daily 日报 / weekly 周报。"
                },
                "date_from": {
                    "type": "string",
                    "description": "起始日期 YYYY-MM-DD,可选。"
                },
                "date_to": {
                    "type": "string",
                    "description": "结束日期 YYYY-MM-DD,可选。"
                },
                "author_mode": {
                    "type": "string",
                    "enum": ["all", "me"],
                    "description": "提交作者范围:all 全部 / me 仅当前 git 用户,可选。"
                }
            },
            "required": ["period_type"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            let project_path = ctx.project_path.clone();
            move |args, on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let period_type = match arg_str(&args, "period_type") {
                        Some("daily") => "daily",
                        Some("weekly") => "weekly",
                        _ => return Err(tool_err(invalid_arg("period_type (daily|weekly)"))),
                    }
                    .to_string();
                    let today = Local::now().date_naive();
                    let default_from = if period_type == "weekly" {
                        today - chrono::Duration::days(6)
                    } else {
                        today
                    };
                    let date_from = arg_str(&args, "date_from")
                        .map(str::to_string)
                        .unwrap_or_else(|| default_from.format("%Y-%m-%d").to_string());
                    let date_to = arg_str(&args, "date_to")
                        .map(str::to_string)
                        .unwrap_or_else(|| today.format("%Y-%m-%d").to_string());
                    let range_label = if period_type == "weekly" && date_from != date_to {
                        format!("{date_from} ~ {date_to}")
                    } else {
                        date_from.clone()
                    };
                    let author_mode = match arg_str(&args, "author_mode") {
                        Some("me") => "me".to_string(),
                        _ => "all".to_string(),
                    };
                    let Some(project_id) = project_id else {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::ProjectNotFound,
                            project_path,
                        )));
                    };
                    if let Some(on_update) = &on_update {
                        on_update(AgentToolResult::text("正在收集提交并生成报告…"));
                    }
                    // GenerateAndSaveReportRequest / GeneratedReport 的字段当前是模块私有,
                    // 对齐阶段需放宽为 pub(crate)(见对齐清单)。
                    let request = GenerateAndSaveReportRequest {
                        run_id: pseudo_request_id(),
                        project_ids: vec![project_id],
                        date_from,
                        date_to,
                        range_label,
                        author_mode,
                        language: "zh-CN".to_string(),
                        period_type,
                    };
                    let db = app.state::<Db>();
                    match ai_generate_and_save_report(app.clone(), db, request)
                        .await
                        .map_err(tool_err)?
                    {
                        None => text_result("所选时间范围内没有提交记录,未生成报告。"),
                        Some(report) => text_result(truncate_bytes(
                            format!(
                                "报告已生成并保存(历史 id:{})。\n\n{}",
                                report.history_id, report.result
                            ),
                            REPORT_RESULT_MAX_BYTES,
                        )),
                    }
                })
            }
        },
    )
}

fn list_reports_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "list_reports",
        "报告历史",
        "列出本项目最近的报告历史(按生成时间倒序)。用户问「之前的报告」「上次周报」时使用,把条目信息(时间范围/类型/提交数)转述给用户即可。参数:limit(可选)返回条数,默认 10。",
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "返回条数(1-50),默认 10。"
                }
            },
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_id = ctx.project_id;
            move |args, _on_update| {
                let app = app.clone();
                Box::pin(async move {
                    let limit = arg_u64_opt(&args, "limit")?
                        .unwrap_or(10)
                        .clamp(1, 50) as usize;
                    let db = app.state::<Db>();
                    let items = report::list_report_history(db, Some(limit), Some(0), project_id)
                        .map_err(tool_err)?;
                    if items.is_empty() {
                        return text_result("本项目还没有报告历史。");
                    }
                    text_result(
                        items
                            .iter()
                            .map(|item| {
                                let generated_at = Local
                                    .timestamp_opt(item.created_at, 0)
                                    .single()
                                    .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_default();
                                let kind = if item.period_type == "weekly" {
                                    "周报"
                                } else {
                                    "日报"
                                };
                                format!(
                                    "- [{}] {}({},{},{} 条提交,生成于 {generated_at})",
                                    item.id,
                                    item.range_label,
                                    kind,
                                    item.project_names.join("、"),
                                    item.total_commits
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
            }
        },
    )
}

// ── 文件读取 ─────────────────────────────────────────────────────────

fn read_project_file_tool(ctx: &ChatToolContext) -> AgentTool {
    tool(
        "read_project_file",
        "读项目文件",
        "读取项目内单个文本文件的指定行区间(带 1-based 行号前缀)。sem_context 的结果不够、或需要查看完整文件/配置时使用;只读工具,路径必须是仓库内相对路径(/ 分隔)。参数:path(必填)仓库相对路径,如 src/lib/ai.ts;offset_line(可选)起始行,默认 1;max_lines(可选)最多返回行数,默认 400(上限 5000)。",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "仓库内相对路径(/ 分隔),如 src/lib/ai.ts。"
                },
                "offset_line": {
                    "type": "integer",
                    "description": "起始行(1-based),默认 1。"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "最多返回行数,默认 400,上限 5000。"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        false,
        {
            let root = ctx.work_dir();
            move |args, _on_update| {
                let root = root.clone();
                Box::pin(async move {
                    let rel_path = to_forward_slash_str(require_str(&args, "path")?.trim());
                    let offset_line = arg_u64_opt(&args, "offset_line")?.unwrap_or(1).max(1);
                    let max_lines = arg_u64_opt(&args, "max_lines")?
                        .unwrap_or(READ_FILE_DEFAULT_LINES)
                        .clamp(1, READ_FILE_MAX_LINES) as usize;
                    // read_file_preview 内部已做 canonicalize + 根目录前缀校验,
                    // 拒绝越界路径与符号链接逃逸。
                    let preview = read_file_preview(root.clone(), rel_path.clone())
                        .map_err(tool_err)?;
                    let Some(text) = preview.text else {
                        return text_result(format!(
                            "「{rel_path}」是二进制或非 UTF-8 文件,无法按行读取。"
                        ));
                    };
                    let lines: Vec<&str> = text.lines().collect();
                    let total = lines.len();
                    let start = ((offset_line as usize).saturating_sub(1)).min(total);
                    if start >= total {
                        return text_result(format!(
                            "文件共 {total} 行,offset_line={offset_line} 超出范围。"
                        ));
                    }
                    let end = (start + max_lines).min(total);
                    let body = lines[start..end]
                        .iter()
                        .enumerate()
                        .map(|(index, line)| format!("{}: {}", start + index + 1, line))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut out = body;
                    if preview.truncated {
                        out.push_str("\n\n…(文件超过 512KB 预览上限,尾部已被截断)");
                    }
                    if end < total {
                        out.push_str(&format!(
                            "\n\n…(共 {total} 行,以上为第 {}~{} 行;继续读取请传 offset_line={})",
                            start + 1,
                            end,
                            end + 1
                        ));
                    }
                    text_result(truncate_bytes(out, TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}
