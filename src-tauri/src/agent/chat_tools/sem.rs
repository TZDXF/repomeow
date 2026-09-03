use serde_json::{json, Value};
use tauri::AppHandle;
use crate::agent::types::{AgentTool};
use crate::commands::semantic::{semantic_entity_callers, semantic_entity_context, semantic_entity_refs, semantic_find_entities, semantic_worktree_diff};
use super::*;

// ── 语义分析 ─────────────────────────────────────────────────────────

pub(super) fn sem_find_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
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

pub(super) fn sem_context_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
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

pub(super) fn sem_relations_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
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

pub(super) fn sem_diff_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
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


