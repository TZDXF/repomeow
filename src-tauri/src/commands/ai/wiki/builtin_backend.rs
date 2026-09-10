use super::*;

use std::path::{Path, PathBuf};

use crate::agent::harness::agent_harness::AgentHarness;
use crate::agent::harness::events::{HarnessEvent, HarnessEventType, ToolEventPhase};
use crate::agent::harness::restricted_env::RestrictedEnv;
use crate::agent::harness::runtime::harness_tool_from_core;
use crate::agent::harness::tools::index::{create_edit_tool, create_write_tool};
use crate::agent::llm::types::{
    AssistantContent, AssistantMessageEvent, Model, ModelThinkingLevel,
};
use crate::agent::types::StreamFn;
use crate::commands::ai::harness_support::{
    assistant_text, builtin_stream_fn, collect_usage_events, create_harness,
    effective_thinking_level, load_builtin_agent_model, prompt_with_timeout, read_tools,
    record_collected_usage,
};
use tokio_util::sync::CancellationToken;

const OUTLINE_READ_BUDGET: usize = 20;
const PAGE_READ_BUDGET: usize = 5;
const MAX_ATTEMPTS: usize = 3;

type ProgressCallback = Arc<dyn Fn(String) + Send + Sync>;
type ActivityCallback = Arc<dyn Fn(String) + Send + Sync>;
type RetryCallback = Arc<dyn Fn(WikiRetryNotice) + Send + Sync>;

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
    let env = RestrictedEnv::for_agent(
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
    record_collected_usage(db, "wiki", &usage_model, &usages.lock().unwrap());
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
    let mut files = wiki::read_wiki_files_in(project_path, &page.relevant_files)?;
    let draft_path = wiki::begin_wiki_page_staging_in(wiki_dir, run_id, page)?;
    let _staging_cleanup = StagingCleanup {
        wiki_dir: wiki_dir.to_path_buf(),
        run_id: run_id.to_string(),
        file_name: page.file.clone(),
    };
    let has_existing_draft =
        !wiki::read_wiki_page_staging_in(wiki_dir, run_id, &page.file)?.is_empty();
    let env = RestrictedEnv::for_agent(project_path, &draft_path)
        .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
    let mut tools = read_tools(env.clone(), PAGE_READ_BUDGET);
    tools.push(harness_tool_from_core(create_write_tool(env.clone())));
    tools.push(harness_tool_from_core(create_edit_tool(env)));
    let tool_schema = tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": tool.name, "description": tool.description, "parameters": tool.parameters,
            })
        })
        .collect::<Vec<_>>();
    let budget = crate::ai::budget::RequestBudget::new(
        &model,
        "You are RepoMeow's built-in coding agent for writing one Wiki page.",
        &serde_json::Value::Array(tool_schema).to_string(),
        None,
    )?;
    // Keep page instructions and paths intact; shrink only source bodies. Missing details
    // remain accessible through the existing read tools and Harness compaction.
    loop {
        let candidate = builtin_agent_wiki_page_prompt(
            page,
            &files,
            changed_files,
            language,
            &draft_path,
            has_existing_draft,
        );
        if budget.fits(&candidate) {
            break;
        }
        let Some(file) = files
            .iter_mut()
            .filter(|file| !file.content.is_empty())
            .max_by_key(|file| file.content.len())
        else {
            return Err(AppError::coded(
                ErrorCode::AiResponseError,
                "Wiki 页面指令超过模型上下文预算",
            ));
        };
        let mut end = file.content.len() / 2;
        while !file.content.is_char_boundary(end) {
            end -= 1;
        }
        file.content.truncate(end);
        file.truncated = true;
    }
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
    record_collected_usage(db, "wiki", &usage_model, &usages.lock().unwrap());
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
