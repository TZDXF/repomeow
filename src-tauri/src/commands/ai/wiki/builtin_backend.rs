use super::*;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::agent::harness::agent_harness::{
    AgentHarness, AgentHarnessOptions, RetryPolicy, RunOutcome,
};
use crate::agent::harness::events::{HarnessEvent, HarnessEventType, ToolEventPhase, UsageEvent};
use crate::agent::harness::restricted_env::RestrictedEnv;
use crate::agent::harness::runtime::harness_tool_from_core;
use crate::agent::harness::session::memory::InMemorySessionStorage;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::SessionMetadata;
use crate::agent::harness::tools::index::{
    create_edit_tool, create_find_tool, create_grep_tool, create_ls_tool, create_read_tool,
    create_write_tool,
};
use crate::agent::harness::types::{AgentHarnessTool, ExecutionEnv};
use crate::agent::harness::uuid::uuid_v7;
use crate::agent::llm::openai_completions::stream_openai_completions;
use crate::agent::llm::types::{
    AssistantContent, AssistantMessage, AssistantMessageEvent, Model, ModelThinkingLevel,
    SimpleStreamOptions, StopReason,
};
use crate::agent::types::{AgentTool, QueueMode, StreamFn, ToolExecutionError, ToolExecutionMode};
use tokio_util::sync::CancellationToken;

const OUTLINE_READ_BUDGET: usize = 20;
const PAGE_READ_BUDGET: usize = 5;
const MAX_ATTEMPTS: usize = 3;
const RUN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;
type ActivityCallback = Arc<dyn Fn(String) + Send + Sync>;
type RetryCallback = Arc<dyn Fn(WikiRetryNotice) + Send + Sync>;

struct BuiltinAgentModel {
    model: Model,
    api_key: String,
}

fn load_builtin_agent_model(app: &AppHandle, chosen: Option<&str>) -> AppResult<BuiltinAgentModel> {
    let file = crate::ai::catalog::load_ai_config_file(app);
    resolve_builtin_model(&file, chosen)
}

/// 内置 Agent 模型解析:显式选择(复合值 "providerId/modelId",模型 id 自身可含
/// "/",按首个 / 拆分)优先;None = 设置页默认模型。所选厂商密钥为空按未配置处理。
fn resolve_builtin_model(
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

fn builtin_stream_fn(api_key: String, cancel: CancellationToken) -> StreamFn {
    Arc::new(move |model, context, options| {
        let api_key = api_key.clone();
        let cancel = cancel.clone();
        Box::pin(async move {
            let base = options.unwrap_or_default();
            stream_openai_completions(
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

fn thinking_level(model: &Model) -> ModelThinkingLevel {
    if model.reasoning {
        ModelThinkingLevel::Medium
    } else {
        ModelThinkingLevel::Off
    }
}

/// 用户显式选择优先;None 回退模型默认(reasoning 中档 / 否则关闭)
fn effective_thinking_level(model: &Model, configured: Option<&str>) -> ModelThinkingLevel {
    configured
        .map(crate::ai::catalog::parse_thinking_level)
        .unwrap_or_else(|| thinking_level(model))
}

fn memory_session() -> Session {
    Session::new(Arc::new(InMemorySessionStorage::new(SessionMetadata {
        id: uuid_v7(),
        created_at: crate::agent::agent_loop::now_ms(),
        parent_session_id: None,
    })))
}

fn budget_tool(tool: AgentTool, budget: Arc<AtomicUsize>, limit: usize) -> AgentTool {
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
                    return Err(ToolExecutionError::from(
                        crate::agent::harness::types::SimpleError::new(format!(
                            "{name} tool budget exceeded: at most {limit} repository exploration calls are allowed"
                        )),
                    ));
                }
                execute(tool_call_id, params, signal, on_update).await
            })
        }),
        ..tool
    }
}

fn read_tools(env: Arc<dyn ExecutionEnv>, limit: usize) -> Vec<AgentHarnessTool> {
    let budget = Arc::new(AtomicUsize::new(0));
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

fn usage_record(db: &Db, model: &str, event: &UsageEvent) {
    let usage = &event.usage;
    let record = AiUsageRecord {
        task_type: "wiki".into(),
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

fn assistant_text(message: &AssistantMessage) -> String {
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

async fn create_harness(
    model: Model,
    stream_fn: StreamFn,
    tools: Vec<AgentHarnessTool>,
    system_prompt: String,
    thinking: ModelThinkingLevel,
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

async fn prompt_with_timeout(
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
                "wiki agent timed out",
            ))
        }
    }
}

async fn collect_usage_events(harness: &AgentHarness) -> Arc<std::sync::Mutex<Vec<UsageEvent>>> {
    let usages = Arc::new(std::sync::Mutex::new(Vec::<UsageEvent>::new()));
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

fn record_collected_usage(db: &Db, model: &str, usages: &[UsageEvent]) {
    for usage in usages {
        usage_record(db, model, usage);
    }
}

pub(super) async fn generate_builtin_outline_pages(
    app: &AppHandle,
    db: &Db,
    context: &wiki::WikiContext,
    project_path: &str,
    project_name: &str,
    language: &str,
    model: Option<&str>,
    thinking: Option<&str>,
    cancel: &CancellationToken,
    on_activity: ActivityCallback,
    on_retry: RetryCallback,
) -> AppResult<(Vec<wiki::WikiOutlinePage>, String)> {
    let configured = load_builtin_agent_model(app, model)?;
    let stream_fn = builtin_stream_fn(configured.api_key.clone(), cancel.child_token());
    let thinking = effective_thinking_level(&configured.model, thinking);
    generate_outline_with(
        db,
        configured.model,
        stream_fn,
        thinking,
        context,
        project_path,
        project_name,
        language,
        cancel,
        on_activity,
        on_retry,
    )
    .await
}

pub(super) async fn generate_outline_with(
    db: &Db,
    model: Model,
    stream_fn: StreamFn,
    thinking: ModelThinkingLevel,
    context: &wiki::WikiContext,
    project_path: &str,
    project_name: &str,
    language: &str,
    cancel: &CancellationToken,
    on_activity: ActivityCallback,
    on_retry: RetryCallback,
) -> AppResult<(Vec<wiki::WikiOutlinePage>, String)> {
    let usage_model = model.id.clone();
    let request_cancel = cancel.child_token();
    // 大纲任务无写需求:允许写目标指向一个项目内不存在的占位路径,
    // 受限环境因此事实上只读(写工具也不注册)。
    let env = RestrictedEnv::for_wiki_agent(
        project_path,
        Path::new(project_path).join(".repomeow-outline-no-write"),
    )
    .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
    let harness = Arc::new(
        create_harness(
            model,
            stream_fn,
            read_tools(env, OUTLINE_READ_BUDGET),
            "You are RepoMeow's built-in coding agent for Wiki outline generation.".to_string(),
            thinking,
        )
        .await?,
    );
    let usages = collect_usage_events(&harness).await;
    let activity = on_activity.clone();
    let _tool_subscription = harness.on_event(
        HarnessEventType::Tool,
        Arc::new(move |event| {
            if let HarnessEvent::Tool(event) = event {
                if event.phase == ToolEventPhase::Start {
                    activity(event.tool_name.clone());
                }
            }
        }),
    );

    let original_prompt = agent_wiki_outline_prompt(context, project_name, language);
    let valid_files: HashSet<String> = context.paths.iter().cloned().collect();
    let mut prompt = original_prompt.clone();
    let mut last_error = "wiki outline JSON was not generated".to_string();
    let result = async {
        for attempt in 1..=MAX_ATTEMPTS {
            let message =
                prompt_with_timeout(harness.clone(), prompt.clone(), cancel, &request_cancel)
                    .await?;
            let text = sdk::strip_thinking(&assistant_text(&message));
            match crate::ai::wiki_outline::parse_outline(&text, &valid_files) {
                Ok(pages) => return Ok(pages),
                Err(error) => {
                    last_error = error;
                    if attempt < MAX_ATTEMPTS {
                        on_retry(WikiRetryNotice {
                            attempt,
                            max_attempts: MAX_ATTEMPTS,
                            delay_seconds: 0,
                            reason: "temporary".into(),
                        });
                        prompt = outline_retry_prompt(&original_prompt, &last_error);
                    }
                }
            }
        }
        Err(AppError::coded(
            ErrorCode::AiResponseParseFailed,
            format!("wiki outline: {last_error}"),
        ))
    }
    .await;
    record_collected_usage(db, &usage_model, &usages.lock().unwrap());
    result.map(|pages| (pages, usage_model))
}

pub(super) async fn generate_builtin_page_to_disk(
    app: &AppHandle,
    db: &Db,
    run_id: &str,
    project_path: &str,
    page: &wiki::WikiOutlinePage,
    language: &str,
    changed_files: &[String],
    model: Option<&str>,
    thinking: Option<&str>,
    cancel: &CancellationToken,
    on_progress: ProgressCallback,
    on_activity: ActivityCallback,
    on_retry: RetryCallback,
) -> AppResult<String> {
    let configured = load_builtin_agent_model(app, model)?;
    let stream_fn = builtin_stream_fn(configured.api_key.clone(), cancel.child_token());
    let wiki_dir = wiki::wiki_dir(app, project_path)?;
    let thinking = effective_thinking_level(&configured.model, thinking);
    generate_page_with(
        db,
        configured.model,
        stream_fn,
        thinking,
        run_id,
        project_path,
        &wiki_dir,
        page,
        language,
        changed_files,
        cancel,
        on_progress,
        on_activity,
        on_retry,
    )
    .await
}

/// Agent 直接写入的暂存清理守卫:暂存创建后任何路径失败/取消都清走草稿,
/// 成功提升后清理为幂等空操作。
struct StagingCleanup {
    wiki_dir: PathBuf,
    run_id: String,
    file_name: String,
}

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = wiki::cancel_wiki_page_staging_in(&self.wiki_dir, &self.run_id, &self.file_name);
    }
}

pub(super) async fn generate_page_with(
    db: &Db,
    model: Model,
    stream_fn: StreamFn,
    thinking: ModelThinkingLevel,
    run_id: &str,
    project_path: &str,
    wiki_dir: &Path,
    page: &wiki::WikiOutlinePage,
    language: &str,
    changed_files: &[String],
    cancel: &CancellationToken,
    on_progress: ProgressCallback,
    on_activity: ActivityCallback,
    on_retry: RetryCallback,
) -> AppResult<String> {
    let usage_model = model.id.clone();
    let request_cancel = cancel.child_token();
    let files = wiki::read_wiki_files_in(project_path, &page.relevant_files)?;
    let draft_path = wiki::begin_wiki_page_staging_in(wiki_dir, run_id, page)?;
    let _staging_cleanup = StagingCleanup {
        wiki_dir: wiki_dir.to_path_buf(),
        run_id: run_id.to_string(),
        file_name: page.file.clone(),
    };
    let has_existing_draft =
        !wiki::read_wiki_page_staging_in(wiki_dir, run_id, &page.file)?.is_empty();
    let env = RestrictedEnv::for_wiki_agent(project_path, &draft_path)
        .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
    let mut tools = read_tools(env.clone(), PAGE_READ_BUDGET);
    tools.push(harness_tool_from_core(create_write_tool(env.clone())));
    tools.push(harness_tool_from_core(create_edit_tool(env)));
    let harness = Arc::new(
        create_harness(
            model,
            stream_fn,
            tools,
            "You are RepoMeow's built-in coding agent for writing one Wiki page.".to_string(),
            thinking,
        )
        .await?,
    );
    let usages = collect_usage_events(&harness).await;

    // 实时预览单一事实源:write 工具参数流中正在成形的 content;
    // edit 完成后重读暂存文件刷新(暂存内容即最新全量)。
    let preview = on_progress.clone();
    let preview_path = draft_path.clone();
    let preview_subscription = harness.on_event(
        HarnessEventType::MessageUpdate,
        Arc::new(move |event| {
            let HarnessEvent::MessageUpdate(event) = event else {
                return;
            };
            let (AssistantMessageEvent::ToolcallDelta { partial, .. }
            | AssistantMessageEvent::ToolcallEnd { partial, .. }) = &event.assistant_message_event
            else {
                return;
            };
            for content in partial.content.iter().rev() {
                let AssistantContent::ToolCall(call) = content else {
                    continue;
                };
                if call.name == "write"
                    && call
                        .arguments
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        == Some(preview_path.as_str())
                {
                    if let Some(content) = call
                        .arguments
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                    {
                        preview(content.to_string());
                    }
                    break;
                }
            }
        }),
    );
    let activity = on_activity.clone();
    let preview_after_edit = on_progress.clone();
    let edit_wiki_dir = wiki_dir.to_path_buf();
    let edit_run_id = run_id.to_string();
    let edit_file = page.file.clone();
    let tool_subscription = harness.on_event(
        HarnessEventType::Tool,
        Arc::new(move |event| {
            let HarnessEvent::Tool(event) = event else {
                return;
            };
            if event.phase == ToolEventPhase::Start {
                activity(event.tool_name.clone());
            } else if event.phase == ToolEventPhase::End
                && event.tool_name == "edit"
                && !event.is_error
            {
                if let Ok(content) =
                    wiki::read_wiki_page_staging_in(&edit_wiki_dir, &edit_run_id, &edit_file)
                {
                    preview_after_edit(content);
                }
            }
        }),
    );

    let prompt = builtin_agent_wiki_page_prompt(
        page,
        &files,
        changed_files,
        language,
        &draft_path,
        has_existing_draft,
    );
    let result = async {
        prompt_with_timeout(harness.clone(), prompt, cancel, &request_cancel).await?;
        promote_with_repair(
            &harness,
            wiki_dir,
            run_id,
            project_path,
            page,
            cancel,
            &request_cancel,
            &on_retry,
        )
        .await
    }
    .await;
    preview_subscription();
    tool_subscription();
    record_collected_usage(db, &usage_model, &usages.lock().unwrap());
    result.map(|()| usage_model)
}

/// 提升暂存页;校验失败时给同一会话一次明确修复机会,再失败则不替换正式页
async fn promote_with_repair(
    harness: &Arc<AgentHarness>,
    wiki_dir: &Path,
    run_id: &str,
    project_path: &str,
    page: &wiki::WikiOutlinePage,
    cancel: &CancellationToken,
    request_cancel: &CancellationToken,
    on_retry: &RetryCallback,
) -> AppResult<()> {
    match wiki::promote_wiki_page_staging_in(wiki_dir, project_path, run_id, page) {
        Ok(()) => Ok(()),
        Err(first_error) if !cancel.is_cancelled() => {
            on_retry(WikiRetryNotice {
                attempt: 1,
                max_attempts: 2,
                delay_seconds: 0,
                reason: "temporary".into(),
            });
            let repair = format!(
                "The writable draft failed validation. Exact validation error:\n<validation_error>\n{}\n</validation_error>\nRead the draft, repair it in place with write or edit, re-run every acceptance check, and finish only after the file is valid.",
                first_error
            );
            prompt_with_timeout(harness.clone(), repair, cancel, request_cancel).await?;
            wiki::promote_wiki_page_staging_in(wiki_dir, project_path, run_id, page)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod thinking_level_tests {
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
