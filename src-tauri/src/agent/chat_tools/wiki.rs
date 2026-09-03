use serde_json::{json};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use crate::agent::types::{AgentTool, AgentToolResult};
use crate::commands::ai::{ai_generate_wiki, ai_update_wiki, GenerateWikiRequest, UpdateWikiRequest, WikiGenerationBackend, WikiGenerationEvent, WikiUpdateEvent};
use crate::commands::wiki::{load_wiki, load_wiki_config_internal, WikiOutlinePage};
use crate::db::Db;
use crate::error::{AppError, ErrorCode};
use super::*;

// ── Wiki ─────────────────────────────────────────────────────────────

pub(super) fn page_list(pages: &[WikiOutlinePage]) -> String {
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

pub(super) fn read_wiki_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
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
                            "该项目尚未生成 Wiki。不要擅自生成;可告知用户,仅在用户明确要求时才用 regenerate_wiki 生成整本(后台执行,耗时较长;当前为「确认后执行」权限时,应用会在执行前弹出确认)。",
                        );
                    };
                    match arg_str(&args, "page_id") {
                        Some(page_id) => match data.pages.iter().find(|page| page.id == page_id) {
                            Some(page) => {
                                let stale_prefix = if data.stale {
                                    "(注意:本 Wiki 已落后于最新代码,以下内容可能过时。)\n\n"
                                } else {
                                    ""
                                };
                                text_result(truncate_bytes(
                                    format!("{stale_prefix}## {}\n\n{}", page.title, page.content),
                                    WIKI_PAGE_MAX_BYTES,
                                ))
                            }
                            None => text_result(format!(
                                "未找到页面 id「{page_id}」。当前页面清单:\n{}",
                                page_list(&data.meta.outline)
                            )),
                        },
                        None => {
                            let stale_note = if data.stale {
                                "是(代码已更新,Wiki 可能过时。不要自行更新:先基于现有内容回答并告知用户可能过时;确有更新必要时再调用 update_wiki,当前为「确认后执行」权限时,应用会在执行前弹出确认)"
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

pub(super) fn update_wiki_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "update_wiki",
        "增量更新 Wiki",
        "对已有 Wiki 做增量更新:检测自上次生成以来的代码变更,仅重新生成受影响页面(同步等待完成,通常几秒到一两分钟)。仅在 Wiki 过时(read_wiki 返回「过时:是」)且确有更新必要时调用;当前为「确认后执行」权限时,应用会在执行前弹出确认,不必在正文中先征得同意,但不要自动触发。项目还没有 Wiki 或历史被改写时本工具会报错,此时改用 regenerate_wiki。若失败原因是「配置的模型不存在 / model not found」,先用 get_ai_config 查看可用模型、询问用户选用哪个,再用 set_wiki_model 写回 Wiki 配置后重试本工具。无参数。",
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

pub(super) fn regenerate_wiki_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "regenerate_wiki",
        "整本重生成 Wiki",
        "在后台启动整本 Wiki 重新生成(立即返回,不等待完成;耗时几分钟到几十分钟)。**仅在用户明确要求整本重生成时使用**(首次生成或重写整本);当前为「确认后执行」权限时,应用会在执行前弹出确认,不必在正文中先征得同意。不要为了更新少数几页而使用它——那是 update_wiki 的职责。若报错「配置的模型不存在 / model not found」,先用 get_ai_config 查看可用模型、询问用户选用哪个,再用 set_wiki_model 写回 Wiki 配置后重试本工具。无参数。",
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
                    // 后台任务失败只会进 stderr,agent 无从感知;启动前先预检内置
                    // 后端模型(配置缺失/模型不存在时直接报错,引导走
                    // get_ai_config + set_wiki_model 的换模型恢复流程)。
                    let wiki_config = load_wiki_config_internal(&app, &project_path)
                        .map_err(tool_err)?;
                    if let WikiGenerationBackend::Builtin { model, .. } = &wiki_config.backend {
                        let ai_config = crate::ai::catalog::load_ai_config_file(&app);
                        let (_, status) = builtin_model_status(&ai_config, model.as_deref());
                        if let Err(reason) = status {
                            return Err(tool_err(AppError::coded(
                                ErrorCode::AiNotConfigured,
                                format!(
                                    "{reason}。可用 get_ai_config 查看可用模型,与用户确认后用 set_wiki_model 切换再重试"
                                ),
                            )));
                        }
                    }
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


