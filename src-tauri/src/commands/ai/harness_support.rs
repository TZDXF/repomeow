//! 内置 AgentHarness 的无头组装胶水(wiki 生成与资源库安全扫描共享):
//! 内存会话、provider 流桥接、只读工具调用预算、harness 构造、prompt
//! 超时/取消与逐请求用量采集。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::agent::harness::agent_harness::{
    AgentHarness, AgentHarnessOptions, RetryPolicy, RunOutcome,
};
use crate::agent::harness::events::{HarnessEvent, HarnessEventType, UsageEvent};
use crate::agent::harness::runtime::harness_tool_from_core;
use crate::agent::harness::session::memory::InMemorySessionStorage;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::SessionMetadata;
use crate::agent::harness::tools::index::{
    create_find_tool, create_grep_tool, create_ls_tool, create_read_tool,
};
use crate::agent::harness::types::{AgentHarnessTool, ExecutionEnv, SimpleError};
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::stream_simple;
use crate::agent::llm::types::{
    AssistantContent, AssistantMessage, Model, ModelThinkingLevel, SimpleStreamOptions, StopReason,
};
use crate::agent::types::{AgentTool, QueueMode, StreamFn, ToolExecutionError, ToolExecutionMode};
use crate::commands::usage::insert_usage_row;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::AiUsageRecord;
use crate::time_util::now_ts;

/// 单次 agent prompt 的总超时(与 ACP prompt 上限一致)
const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub(crate) fn memory_session() -> Session {
    Session::new(Arc::new(InMemorySessionStorage::new(SessionMetadata {
        id: uuid_v7(),
        created_at: crate::agent::agent_loop::now_ms(),
        parent_session_id: None,
    })))
}

pub(crate) fn builtin_stream_fn(api_key: String, cancel: CancellationToken) -> StreamFn {
    Arc::new(move |model, context, options| {
        let api_key = api_key.clone();
        let cancel = cancel.clone();
        Box::pin(async move {
            let base = options.unwrap_or_default();
            stream_simple(
                model,
                context,
                Some(SimpleStreamOptions {
                    api_key: Some(api_key),
                    ..base
                }),
                Some(cancel),
            )
        })
    })
}

/// 内置 Agent 模型解析结果
pub(crate) struct BuiltinAgentModel {
    pub model: Model,
    pub api_key: String,
}

pub(crate) fn load_builtin_agent_model(
    app: &tauri::AppHandle,
    chosen: Option<&str>,
) -> AppResult<BuiltinAgentModel> {
    let file = crate::ai::catalog::load_ai_config_file(app);
    resolve_builtin_model(&file, chosen)
}

/// 内置 Agent 模型解析:显式选择(复合值 "providerId/modelId",模型 id 自身可含
/// "/",按首个 / 拆分)优先;None = 设置页默认模型。所选厂商密钥为空按未配置处理。
pub(crate) fn resolve_builtin_model(
    file: &crate::ai::catalog::AiConfigFile,
    chosen: Option<&str>,
) -> AppResult<BuiltinAgentModel> {
    let (provider_id, model_id) = match chosen {
        None => {
            let (model, api_key) = crate::ai::catalog::resolve_default_model(file)?;
            return Ok(BuiltinAgentModel { model, api_key });
        }
        Some(value) => value.split_once('/').ok_or_else(|| {
            AppError::coded(
                ErrorCode::AiNotConfigured,
                format!("invalid model reference: {value}"),
            )
        })?,
    };
    let model = crate::ai::catalog::resolve_model(file, provider_id, model_id)?;
    let api_key = file
        .providers
        .get(provider_id)
        .map(|provider| provider.api_key.trim().to_string())
        .unwrap_or_default();
    if api_key.is_empty() {
        return Err(AppError::coded(ErrorCode::AiNotConfigured, ""));
    }
    Ok(BuiltinAgentModel { model, api_key })
}

fn thinking_level(model: &Model) -> ModelThinkingLevel {
    if model.reasoning {
        ModelThinkingLevel::Medium
    } else {
        ModelThinkingLevel::Off
    }
}

/// 用户显式选择优先;None 回退模型默认(reasoning 中档 / 否则关闭)
pub(crate) fn effective_thinking_level(
    model: &Model,
    configured: Option<&str>,
) -> ModelThinkingLevel {
    configured
        .map(crate::ai::catalog::parse_thinking_level)
        .unwrap_or_else(|| thinking_level(model))
}

fn budget_tool(
    tool: AgentTool,
    budget: Arc<std::sync::atomic::AtomicUsize>,
    limit: usize,
) -> AgentTool {
    use std::sync::atomic::Ordering;

    let execute = tool.execute.clone();
    let name = tool.name.clone();
    AgentTool {
        execute: Arc::new(move |tool_call_id, params, signal, on_update| {
            let execute = execute.clone();
            let budget = budget.clone();
            let name = name.clone();
            Box::pin(async move {
                let used = budget.fetch_add(1, Ordering::SeqCst) + 1;
                if used > limit {
                    return Err(ToolExecutionError::from(SimpleError::new(format!(
                        "{name} tool budget exceeded: at most {limit} repository exploration calls are allowed"
                    ))));
                }
                execute(tool_call_id, params, signal, on_update).await
            })
        }),
        ..tool
    }
}

/// 只读工具集(read/grep/find/ls),统一包装调用次数预算
pub(crate) fn read_tools(env: Arc<dyn ExecutionEnv>, limit: usize) -> Vec<AgentHarnessTool> {
    let budget = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    [
        create_read_tool(env.clone(), None),
        create_grep_tool(env.clone()),
        create_find_tool(env.clone()),
        create_ls_tool(env),
    ]
    .into_iter()
    .map(|tool| budget_tool(tool, budget.clone(), limit))
    .map(harness_tool_from_core)
    .collect()
}

/// 逐 LLM 请求落库(harness 每轮 assistant 消息一个 UsageEvent)
pub(crate) fn usage_record(db: &Db, task_type: &str, model: &str, event: &UsageEvent) {
    let usage = &event.usage;
    let record = AiUsageRecord {
        task_type: task_type.to_string(),
        model: model.to_string(),
        input_tokens: Some(usage.input),
        output_tokens: Some(usage.output),
        total_tokens: Some(usage.total_tokens),
        duration_ms: event.elapsed_ms,
        cached_tokens: Some(usage.cache_read),
    };
    if let Ok(conn) = db.0.lock() {
        let _ = insert_usage_row(&conn, &record, now_ts());
    }
}

pub(crate) fn record_collected_usage(db: &Db, task_type: &str, model: &str, usages: &[UsageEvent]) {
    for usage in usages {
        usage_record(db, task_type, model, usage);
    }
}

pub(crate) fn assistant_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_outcome_message(outcome: RunOutcome) -> AppResult<AssistantMessage> {
    match outcome {
        RunOutcome::Completed { final_message, .. }
            if !matches!(
                final_message.stop_reason,
                StopReason::Error | StopReason::Aborted | StopReason::Length
            ) =>
        {
            Ok(final_message)
        }
        RunOutcome::Completed { final_message, .. } | RunOutcome::Aborted { final_message, .. } => {
            Err(AppError::coded(
                ErrorCode::AiRequestFailed,
                final_message
                    .error_message
                    .unwrap_or_else(|| format!("agent stopped: {:?}", final_message.stop_reason)),
            ))
        }
        RunOutcome::Failed { error, .. } => {
            Err(AppError::coded(ErrorCode::AiRequestFailed, error.message))
        }
        RunOutcome::Suspended { .. } => Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "agent suspended",
        )),
    }
}

pub(crate) async fn create_harness(
    model: Model,
    stream_fn: StreamFn,
    tools: Vec<AgentHarnessTool>,
    system_prompt: String,
    thinking: crate::agent::llm::types::ModelThinkingLevel,
) -> AppResult<AgentHarness> {
    let (harness, suspended) = AgentHarness::create(AgentHarnessOptions {
        session: memory_session(),
        stream_fn,
        thinking_level: Some(thinking),
        model,
        active_tool_names: None,
        tools,
        tool_context: None,
        system_prompt: Some(system_prompt),
        resources: Default::default(),
        stream_options: Default::default(),
        retry: Some(RetryPolicy {
            enabled: true,
            max_retries: 2,
            base_delay_ms: 1000,
        }),
        compaction: None,
        steering_mode: QueueMode::OneAtATime,
        follow_up_mode: QueueMode::OneAtATime,
        tool_execution: ToolExecutionMode::Sequential,
        telemetry_context: None,
    })
    .await
    .map_err(|error| AppError::coded(ErrorCode::AiRequestFailed, error.to_string()))?;
    if !suspended.is_empty() {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "unexpected suspended in-memory session",
        ));
    }
    Ok(harness)
}

/// prompt 执行:`cancel` 触发 abort,或总超时;返回最终 assistant 消息
pub(crate) async fn prompt_with_timeout(
    harness: Arc<AgentHarness>,
    prompt: String,
    cancel: &CancellationToken,
    request_cancel: &CancellationToken,
) -> AppResult<AssistantMessage> {
    let abort_harness = harness.clone();
    let cancel_watch = cancel.clone();
    let request_watch = request_cancel.clone();
    let watcher = tokio::spawn(async move {
        cancel_watch.cancelled().await;
        request_watch.cancel();
        let _ = abort_harness.abort().await;
    });
    let result = tokio::time::timeout(RUN_TIMEOUT, harness.prompt(prompt)).await;
    watcher.abort();
    match result {
        Ok(Ok(outcome)) => run_outcome_message(outcome),
        Ok(Err(error)) => Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            error.to_string(),
        )),
        Err(_) => {
            request_cancel.cancel();
            let _ = harness.abort().await;
            Err(AppError::coded(
                ErrorCode::AiRequestFailed,
                "agent timed out",
            ))
        }
    }
}

pub(crate) async fn collect_usage_events(harness: &AgentHarness) -> Arc<Mutex<Vec<UsageEvent>>> {
    let usages = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let listener = usages.clone();
    let _subscription = harness.on_event(
        HarnessEventType::Usage,
        Arc::new(move |event| {
            if let HarnessEvent::Usage(event) = event {
                listener.lock().unwrap().push(event.clone());
            }
        }),
    );
    usages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_thinking_overrides_model_default() {
        let mut model = crate::agent::agent_loop::testing::test_model();
        assert!(!model.reasoning);
        // 未配置:非 reasoning 模型默认关闭
        assert_eq!(
            effective_thinking_level(&model, None),
            ModelThinkingLevel::Off
        );
        // 显式配置优先,未知值按 off 兜底
        assert_eq!(
            effective_thinking_level(&model, Some("high")),
            ModelThinkingLevel::High
        );
        assert_eq!(
            effective_thinking_level(&model, Some("bogus")),
            ModelThinkingLevel::Off
        );
        // reasoning 模型未配置时回退中档
        model.reasoning = true;
        assert_eq!(
            effective_thinking_level(&model, None),
            ModelThinkingLevel::Medium
        );
    }

    fn config_file() -> crate::ai::catalog::AiConfigFile {
        serde_json::from_str::<crate::ai::catalog::AiConfigFile>(
            r#"{
                "version": 1,
                "providers": {
                    "deepseek": {
                        "name": "DeepSeek",
                        "baseUrl": "https://api.deepseek.com",
                        "apiKey": " sk-a ",
                        "api": "openai-completions",
                        "models": [{
                            "id": "deepseek-v4-pro",
                            "name": "DeepSeek V4 Pro",
                            "reasoning": true,
                            "input": ["text"],
                            "contextWindow": 128000,
                            "maxTokens": 8192
                        }]
                    },
                    "zhipuai": {
                        "name": "Zhipu",
                        "baseUrl": "https://open.bigmodel.cn",
                        "apiKey": "sk-b",
                        "api": "openai-completions",
                        "models": [{
                            "id": "glm/ultra",
                            "name": "GLM Ultra",
                            "reasoning": false,
                            "input": ["text"],
                            "contextWindow": 128000,
                            "maxTokens": 8192
                        }]
                    }
                },
                "defaultModel": { "providerId": "deepseek", "modelId": "deepseek-v4-pro" }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn builtin_model_resolution_prefers_explicit_choice() {
        let file = config_file();
        // None → 设置页默认模型
        let fallback = resolve_builtin_model(&file, None).unwrap();
        assert_eq!(fallback.model.id, "deepseek-v4-pro");
        assert_eq!(fallback.api_key, "sk-a");
        // 显式复合值优先;模型 id 含 "/" 时按首个 / 拆分;密钥去空白
        let chosen = resolve_builtin_model(&file, Some("zhipuai/glm/ultra")).unwrap();
        assert_eq!(chosen.model.id, "glm/ultra");
        assert_eq!(chosen.model.base_url, "https://open.bigmodel.cn");
        assert_eq!(chosen.api_key, "sk-b");
        // 缺 / 、未知厂商、未知模型都明确报错
        assert!(resolve_builtin_model(&file, Some("no-slash")).is_err());
        assert!(resolve_builtin_model(&file, Some("ghost/m")).is_err());
        assert!(resolve_builtin_model(&file, Some("zhipuai/none")).is_err());
    }
}
