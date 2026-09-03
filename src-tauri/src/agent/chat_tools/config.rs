use serde_json::{json, Value};
use tauri::AppHandle;
use crate::agent::types::{AgentTool};
use crate::commands::ai::{WikiGenerationBackend};
use crate::commands::wiki::{load_wiki_config_internal};
use crate::error::{AppError, ErrorCode};
use super::*;

// ── AI 配置与 Wiki 模型 ──────────────────────────────────────────────

/// 内置后端实际生效的模型引用:显式配置优先,缺省回退设置页默认模型。
/// 返回 (复合引用 "providerId/modelId", 校验结果);校验失败即 wiki 生成会报
/// 「配置的模型不存在 / 未配置」的原因。
pub(super) fn builtin_model_status(
    config: &crate::ai::catalog::AiConfigFile,
    configured: Option<&str>,
) -> (Option<String>, Result<(), String>) {
    let reference = match configured {
        Some(value) => value.to_string(),
        None => match &config.default_model {
            Some(reference) => format!("{}/{}", reference.provider_id, reference.model_id),
            None => return (None, Err("未配置默认模型".to_string())),
        },
    };
    let Some((provider_id, model_id)) = reference.split_once('/') else {
        return (Some(reference.clone()), Err("模型引用格式非法".to_string()));
    };
    let result = match crate::ai::catalog::resolve_model(config, provider_id, model_id) {
        Err(error) => Err(format!("配置的模型不存在:{reference}({error})")),
        Ok(_) if config
            .providers
            .get(provider_id)
            .is_none_or(|provider| provider.api_key.trim().is_empty()) =>
        {
            Err(format!("厂商 {provider_id} 未配置 API Key"))
        }
        Ok(_) => Ok(()),
    };
    (Some(reference), result)
}

pub(super) fn get_ai_config_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "get_ai_config",
        "AI 配置",
        "查看 AI 接入配置:厂商清单(id/名称/baseUrl/是否已配密钥)及其模型列表(id/名称/是否支持推理)、默认模型、问答面板当前使用的模型,以及本项目 Wiki 生成配置(后端/模型/思考强度/并发)和该模型当前是否有效。回答「有哪些可用模型」「Wiki 用的是哪个模型」时使用;Wiki 生成或更新因「配置的模型不存在」失败时,先用它挑替代模型、询问用户后再用 set_wiki_model 写回。密钥本身不会返回。无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        false,
        {
            let app = app.clone();
            let project_path = ctx.project_path.clone();
            move |_args, _on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let config = crate::ai::catalog::load_ai_config_file(&app);
                    let providers: Vec<Value> = config
                        .providers
                        .iter()
                        .map(|(id, provider)| {
                            json!({
                                "id": id,
                                "name": provider.name,
                                "baseUrl": provider.base_url,
                                "hasApiKey": !provider.api_key.trim().is_empty(),
                                "models": provider
                                    .models
                                    .iter()
                                    .map(|model| json!({
                                        "id": model.id,
                                        "name": model.name,
                                        "reasoning": model.reasoning,
                                    }))
                                    .collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    let wiki_backend = load_wiki_config_internal(&app, &project_path)
                        .map_err(tool_err)?
                        .backend;
                    let wiki = match &wiki_backend {
                        WikiGenerationBackend::Builtin {
                            model,
                            thinking,
                            concurrency,
                        } => {
                            let (effective, status) =
                                builtin_model_status(&config, model.as_deref());
                            json!({
                                "backend": "builtin",
                                "configuredModel": model,
                                "effectiveModel": effective,
                                "modelOk": status.is_ok(),
                                "modelError": status.err(),
                                "thinking": thinking,
                                "concurrency": concurrency,
                            })
                        }
                        WikiGenerationBackend::Agent {
                            agent_id,
                            model,
                            thinking,
                            concurrency,
                            ..
                        } => json!({
                            "backend": "agent",
                            "agentId": agent_id,
                            "model": model,
                            "thinking": thinking,
                            "concurrency": concurrency,
                            "note": "本地 agent 后端的模型由 agent 自身管理,set_wiki_model 不适用",
                        }),
                    };
                    let out = json!({
                        "defaultModel": config.default_model.as_ref().map(|reference| {
                            format!("{}/{}", reference.provider_id, reference.model_id)
                        }),
                        "chat": {
                            "providerId": config.chat.provider_id,
                            "modelId": config.chat.model_id,
                            "thinking": config.chat.thinking,
                        },
                        "providers": providers,
                        "wiki": wiki,
                    });
                    text_result(truncate_bytes(pretty_json(&out), TOOL_RESULT_MAX_BYTES))
                })
            }
        },
    )
}

pub(super) fn set_wiki_model_tool(app: &AppHandle, ctx: &ChatToolContext) -> AgentTool {
    tool(
        "set_wiki_model",
        "切换 Wiki 模型",
        "把本项目 Wiki 生成(内置后端)使用的模型切换为指定模型,写回项目 Wiki 配置。典型用途:Wiki 生成/更新因「配置的模型不存在」失败时,先用 get_ai_config 查看可用模型并与用户确认替代项,再调用本工具,然后重试 update_wiki / regenerate_wiki。当前为「确认后执行」权限时,应用会在执行前弹出确认。仅适用于内置后端;Wiki 配置为本地 agent 后端时会报错(其模型由 agent 自身管理,需在 Wiki 生成对话框调整)。参数:provider_id(必填)与 model_id(必填),取值来自 get_ai_config 的模型清单。",
        json!({
            "type": "object",
            "properties": {
                "provider_id": {
                    "type": "string",
                    "description": "厂商 id(来自 get_ai_config 的 providers[].id)。"
                },
                "model_id": {
                    "type": "string",
                    "description": "模型 id(来自 get_ai_config 的 providers[].models[].id)。"
                }
            },
            "required": ["provider_id", "model_id"],
            "additionalProperties": false
        }),
        true,
        {
            let app = app.clone();
            let project_path = ctx.project_path.clone();
            move |args, _on_update| {
                let app = app.clone();
                let project_path = project_path.clone();
                Box::pin(async move {
                    let provider_id = require_str(&args, "provider_id")?;
                    let model_id = require_str(&args, "model_id")?;
                    let ai_config = crate::ai::catalog::load_ai_config_file(&app);
                    crate::ai::catalog::resolve_model(&ai_config, &provider_id, &model_id)
                        .map_err(tool_err)?;
                    let provider_name = ai_config
                        .providers
                        .get(&provider_id)
                        .map(|provider| provider.name.clone())
                        .unwrap_or_default();
                    if ai_config
                        .providers
                        .get(&provider_id)
                        .is_none_or(|provider| provider.api_key.trim().is_empty())
                    {
                        return Err(tool_err(AppError::coded(
                            ErrorCode::AiNotConfigured,
                            format!("厂商 {provider_id} 未配置 API Key,无法用于 Wiki 生成"),
                        )));
                    }
                    let mut wiki_config = load_wiki_config_internal(&app, &project_path)
                        .map_err(tool_err)?;
                    let WikiGenerationBackend::Builtin { model, .. } = &mut wiki_config.backend
                    else {
                        return text_result(
                            "本项目 Wiki 生成配置的是本地 agent 后端,模型由 agent 自身管理,本工具不适用;请在 Wiki 面板的生成对话框中调整后端或模型。",
                        );
                    };
                    let reference = format!("{provider_id}/{model_id}");
                    *model = Some(reference.clone());
                    crate::commands::wiki::save_wiki_config(
                        app.clone(),
                        project_path.clone(),
                        wiki_config,
                    )
                    .map_err(tool_err)?;
                    let provider_label = if provider_name.is_empty() {
                        provider_id.clone()
                    } else {
                        provider_name
                    };
                    text_result(format!(
                        "已将本项目 Wiki 生成模型切换为 {reference}({provider_label})。现在可以重试 update_wiki 或 regenerate_wiki。"
                    ))
                })
            }
        },
    )
}


