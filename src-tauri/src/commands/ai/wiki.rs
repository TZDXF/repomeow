use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::ai::prompts::{
    fixed_system_prompt, language_name, AGENT_WIKI_OUTLINE_PROMPT, AGENT_WIKI_PAGE_PROMPT,
    DEFAULT_WIKI_OUTLINE_PROMPT, DEFAULT_WIKI_PAGE_PROMPT,
};
use crate::ai::sdk;
use crate::commands::usage::insert_usage_row;
use crate::commands::{agent, usage, wiki};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::AiUsageRecord;
use crate::time_util::now_ts;

use super::run::{record_usage, RegisteredRun};

mod agent_backend;
mod builtin_backend;

use agent_backend::*;
use builtin_backend::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WikiGenerationBackend {
    #[default]
    Builtin,
    Agent {
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        custom_command: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        thinking: Option<String>,
        /// 页面并发数(agent 每页独立会话,可并行生成;None/0 = 默认 2,上限 8)
        #[serde(default)]
        concurrency: Option<usize>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateWikiRequest {
    run_id: String,
    project_path: String,
    project_name: String,
    language: String,
    concurrency: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegenerateWikiPageRequest {
    run_id: String,
    project_path: String,
    language: String,
    page: wiki::WikiOutlinePage,
    #[serde(default)]
    changed_files: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWikiRequest {
    run_id: String,
    project_path: String,
    language: String,
    #[serde(default)]
    automatic: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegeneratedWikiPage {
    model: String,
    generator: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiUpdateEvent {
    completed: usize,
    total: usize,
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WikiGenerationEvent {
    Phase {
        phase: String,
    },
    Page {
        page: wiki::WikiOutlinePage,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// 该页生成耗时(毫秒,done 时上报)
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Progress {
        page_id: String,
        content: String,
    },
    Retry {
        #[serde(skip_serializing_if = "Option::is_none")]
        page_id: Option<String>,
        attempt: usize,
        max_attempts: usize,
        delay_seconds: u64,
        reason: String,
    },
    Context {
        file_count: usize,
        tree_truncated: bool,
        has_readme: bool,
        manifest_count: usize,
    },
    ActivityBatch {
        activity_type: String,
        items: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WikiRetryNotice {
    attempt: usize,
    max_attempts: usize,
    delay_seconds: u64,
    reason: String,
}

fn retry_notice(error: &AppError, attempt: usize, max_attempts: usize) -> WikiRetryNotice {
    // agent 后端的限流信号混在底层错误文本里(stderr/协议错误),按内容识别
    let text = error.to_string().to_lowercase();
    let rate_limited = error.code() == "ai_rate_limited"
        || text.contains("429")
        || text.contains("rate limit")
        || text.contains("too many requests");
    WikiRetryNotice {
        attempt,
        max_attempts,
        delay_seconds: 1_u64 << attempt.min(4),
        reason: if rate_limited {
            "rateLimited".into()
        } else {
            "temporary".into()
        },
    }
}

async fn wait_for_wiki_retry(
    cancel: &CancellationToken,
    notice: &WikiRetryNotice,
) -> AppResult<()> {
    tokio::select! {
        _ = tokio::time::sleep(std::time::Duration::from_secs(notice.delay_seconds)) => Ok(()),
        _ = cancel.cancelled() => Err(AppError::coded(ErrorCode::AiRequestFailed, "canceled")),
    }
}

/// 某些 ACP agent 会把自身的 429 重试提示作为正文 chunk 上报。将该传输状态
/// 从最终 Markdown 中剥离，同时保留结构化信息供进度 UI 展示。
fn sanitize_agent_retry_notices(text: &str) -> (String, Option<WikiRetryNotice>) {
    static COMPLETE: OnceLock<regex::Regex> = OnceLock::new();
    static START: OnceLock<regex::Regex> = OnceLock::new();
    static INCOMPLETE: OnceLock<regex::Regex> = OnceLock::new();
    let complete = COMPLETE.get_or_init(|| {
        regex::Regex::new(
            r"(?is)Retrying\s*\(\s*attempt\s+(\d+)\s*/\s*(\d+)\s*,\s*waiting\s+(\d+)\s*s\s*\)\s*\.\.\.\s*Retry finished,\s*resuming\.\s*",
        )
        .expect("valid ACP retry regex")
    });
    let start = START.get_or_init(|| {
        regex::Regex::new(
            r"(?is)Retrying\s*\(\s*attempt\s+(\d+)\s*/\s*(\d+)\s*,\s*waiting\s+(\d+)\s*s\s*\)\s*\.\.\.",
        )
        .expect("valid ACP retry start regex")
    });
    let incomplete = INCOMPLETE.get_or_init(|| {
        regex::Regex::new(r"(?is)Retrying\s*\(\s*attempt[^\r\n]*$")
            .expect("valid ACP partial retry regex")
    });
    let mut latest = None;
    for captures in start.captures_iter(text) {
        latest = Some(WikiRetryNotice {
            attempt: captures[1].parse().unwrap_or(1),
            max_attempts: captures[2].parse().unwrap_or(3),
            delay_seconds: captures[3].parse().unwrap_or(0),
            reason: "rateLimited".into(),
        });
    }
    let mut cleaned = complete.replace_all(text, "").into_owned();
    if let Some(found) = incomplete.find(&cleaned) {
        cleaned.truncate(found.start());
    }
    (cleaned, latest)
}

fn wiki_outline_user_prompt(context: &wiki::WikiContext, project_name: &str) -> String {
    let manifest_section = if context.manifests.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nManifest files:\n{}",
            context
                .manifests
                .iter()
                .map(|manifest| format!("=== {} ===\n{}", manifest.path, manifest.content))
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    };
    let readme_section = context
        .readme
        .as_ref()
        .map(|readme| format!("\n\nREADME:\n{readme}"))
        .unwrap_or_default();
    let truncated_note = if context.tree_truncated {
        "\n(Note: the file tree was truncated; directory entries like `dir/ (N files)` summarize folded subtrees.)"
    } else {
        ""
    };
    format!(
        "Project: {project_name}\n\nFile tree ({} files):{truncated_note}\n{}{}{}",
        context.file_count, context.file_tree, readme_section, manifest_section
    )
}

/// 相关文件全文区块:逐行 `N: ` 前缀(行级引用用),内置与 agent 后端的页面 prompt 共用
fn wiki_files_section(files: &[wiki::WikiFileContent]) -> String {
    let files_section = files
        .iter()
        .map(|file| {
            let numbered = file
                .content
                .lines()
                .enumerate()
                .map(|(index, line)| format!("{}: {line}", index + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "=== {}{} ===\n{numbered}",
                file.path,
                if file.truncated { " (truncated)" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Source files:\n{}",
        if files_section.is_empty() {
            "(no source files available)"
        } else {
            &files_section
        }
    )
}

fn wiki_page_user_prompt(page: &wiki::WikiOutlinePage, files: &[wiki::WikiFileContent]) -> String {
    format!(
        "Wiki page: {}\nCoverage: {}\n\n{}",
        page.title,
        page.description,
        wiki_files_section(files)
    )
}

fn agent_wiki_outline_prompt(
    context: &wiki::WikiContext,
    project_name: &str,
    language: &str,
) -> String {
    format!(
        "{}\n\nRespond in {}.\n\nProject: {}\n\nPreliminary hints (may be incomplete — verify by exploring the repository yourself):\n{}",
        AGENT_WIKI_OUTLINE_PROMPT.trim(),
        language_name(language),
        project_name,
        wiki_outline_user_prompt(context, project_name)
            .split_once("\n\n")
            .map(|(_, rest)| rest)
            .unwrap_or_default(),
    )
}

fn outline_retry_prompt(original: &str, validation_error: &str) -> String {
    format!(
        "{original}\n\n# Correction required\nThe previous response was rejected by the application's strict JSON validator.\n\nExact validation error:\n<validation_error>\n{validation_error}\n</validation_error>\n\nProduce a completely new, full JSON outline that fixes every reported error. Re-run the acceptance check, then return only the corrected JSON object. Do not discuss the error or the correction."
    )
}

/// agent 页面 prompt(混合模式):相关文件全文直接喂入(与内置后端同预算同行号前缀),
/// agent 仅在不足时少量补读,不再逐文件工具调用
fn agent_wiki_page_prompt(
    page: &wiki::WikiOutlinePage,
    files: &[wiki::WikiFileContent],
    changed_files: &[String],
    language: &str,
) -> String {
    let changed = if changed_files.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRecently changed files (this page is being refreshed after these changes):\n{}",
            changed_files
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "{}\n\nRespond in {}.\n\nWiki page: {}\nCoverage: {}{}\n\n{}",
        AGENT_WIKI_PAGE_PROMPT.trim(),
        language_name(language),
        page.title,
        page.description,
        changed,
        wiki_files_section(files),
    )
}

fn record_acp_usage(
    db: &Db,
    model: &str,
    prompt: &str,
    result: &agent::AcpPromptResult,
    duration_ms: i64,
) {
    let (input_tokens, output_tokens, total_tokens, cached_tokens) = match result.usage {
        Some(usage) => (
            i64::try_from(usage.input_tokens).ok(),
            i64::try_from(usage.output_tokens).ok(),
            i64::try_from(usage.total_tokens).ok(),
            usage
                .cached_read_tokens
                .and_then(|value| i64::try_from(value).ok()),
        ),
        None => {
            let input = usage::estimate_text_tokens(model, prompt);
            let output = usage::estimate_text_tokens(model, &result.text);
            (
                Some(input),
                Some(output),
                Some(input.saturating_add(output)),
                None,
            )
        }
    };
    let record = AiUsageRecord {
        task_type: "wiki".into(),
        model: model.to_string(),
        input_tokens,
        output_tokens,
        total_tokens,
        duration_ms: Some(duration_ms),
        cached_tokens,
    };
    if let Ok(conn) = db.0.lock() {
        let _ = insert_usage_row(&conn, &record, now_ts());
    }
}

fn send_wiki_event(channel: &Channel<WikiGenerationEvent>, event: WikiGenerationEvent) {
    let _ = channel.send(event);
}

fn fail_wiki_generation(
    channel: &Channel<WikiGenerationEvent>,
    cancel: &CancellationToken,
    error: AppError,
) -> AppResult<()> {
    send_wiki_event(
        channel,
        WikiGenerationEvent::Phase {
            phase: if cancel.is_cancelled() {
                "cancelled".into()
            } else {
                "failed".into()
            },
        },
    );
    Err(error)
}

fn wiki_backend_id(backend: &WikiGenerationBackend) -> String {
    match backend {
        WikiGenerationBackend::Builtin => "builtin".into(),
        WikiGenerationBackend::Agent { agent_id, .. } => {
            format!("acp:{}", agent_id.as_deref().unwrap_or("custom"))
        }
    }
}

fn should_reject_wiki_backend_change(
    previous_backend: Option<&str>,
    current_backend: &str,
    automatic: bool,
) -> bool {
    !automatic && previous_backend.unwrap_or("builtin") != current_backend
}

/// 整本 Wiki 的收集、大纲、并发/顺序页生成、重试与最终落盘全部在后端执行。
#[tauri::command]
pub async fn ai_generate_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateWikiRequest,
    on_event: Channel<WikiGenerationEvent>,
) -> AppResult<()> {
    let run = RegisteredRun::new(request.run_id);
    let backend = wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "collecting".into(),
        },
    );
    let context = match wiki::collect_wiki_context(request.project_path.clone()) {
        Ok(context) => context,
        Err(error) => return fail_wiki_generation(&on_event, &run.token, error),
    };
    for paths in context.paths.chunks(24) {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::ActivityBatch {
                activity_type: "scan".into(),
                items: paths.to_vec(),
            },
        );
    }
    let mut read_files = context
        .manifests
        .iter()
        .map(|manifest| manifest.path.clone())
        .collect::<Vec<_>>();
    if context.readme.is_some() {
        read_files.insert(0, "README".into());
    }
    if !read_files.is_empty() {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::ActivityBatch {
                activity_type: "read".into(),
                items: read_files,
            },
        );
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Context {
            file_count: context.file_count,
            tree_truncated: context.tree_truncated,
            has_readme: context.readme.is_some(),
            manifest_count: context.manifests.len(),
        },
    );
    let backend_id;
    let meta_model;
    let mut agent_params = None;
    let mut agent_slot = None;
    let mut agent_cancel_watch = None;

    let pages_result = match &backend {
        WikiGenerationBackend::Builtin => {
            backend_id = "builtin".to_string();
            meta_model = sdk::load_config(&app).ai_model;
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: "outlining".into(),
                },
            );
            generate_builtin_outline_pages(
                &app,
                &db,
                &context,
                &request.project_name,
                &request.language,
                &run.token,
                {
                    let channel = on_event.clone();
                    move |notice| {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Retry {
                                page_id: None,
                                attempt: notice.attempt,
                                max_attempts: notice.max_attempts,
                                delay_seconds: notice.delay_seconds,
                                reason: notice.reason,
                            },
                        );
                    }
                },
            )
            .await
        }
        WikiGenerationBackend::Agent { .. } => {
            let params = AgentSessionParams::from_backend(&backend, &request.project_path)
                .expect("agent backend");
            let slot = AgentSessionSlot::default();
            agent_cancel_watch = Some(watch_agent_cancel(run.token.clone(), slot.clone()));
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: "outlining".into(),
                },
            );
            // 大纲用一个独立会话(纠错重试复用同一会话以保留上下文);
            // 页面生成在下方逐页另起会话
            let started = match open_agent_session(&params, &slot).await {
                Ok(started) => started,
                Err(error) => {
                    if let Some(watch) = agent_cancel_watch.take() {
                        watch.abort();
                    }
                    return fail_wiki_generation(&on_event, &run.token, error);
                }
            };
            backend_id = format!("acp:{}", params.agent_id.as_deref().unwrap_or("custom"));
            meta_model = params.usage_model(&started.agent_name);
            let result = generate_agent_outline_pages(
                &db,
                &started.run_id,
                &meta_model,
                &context,
                &request.project_name,
                &request.language,
                {
                    let channel = on_event.clone();
                    Arc::new(move |text| {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::ActivityBatch {
                                activity_type: "tool".into(),
                                items: vec![text],
                            },
                        );
                    })
                },
                {
                    let channel = on_event.clone();
                    Arc::new(move |notice: WikiRetryNotice| {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Retry {
                                page_id: None,
                                attempt: notice.attempt,
                                max_attempts: notice.max_attempts,
                                delay_seconds: notice.delay_seconds,
                                reason: notice.reason,
                            },
                        );
                    })
                },
            )
            .await;
            close_agent_session(&slot, &started.run_id);
            agent_params = Some(params);
            agent_slot = Some(slot);
            result
        }
    };

    let pages = match pages_result {
        Ok(pages) => pages,
        Err(error) => {
            if let Some(watch) = agent_cancel_watch.take() {
                watch.abort();
            }
            if let Some(slot) = &agent_slot {
                slot.cancel_all();
            }
            let phase = if run.token.is_cancelled() {
                "cancelled"
            } else {
                "failed"
            };
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: phase.into(),
                },
            );
            return Err(error);
        }
    };
    if run.token.is_cancelled() {
        if let Some(watch) = agent_cancel_watch.take() {
            watch.abort();
        }
        if let Some(slot) = &agent_slot {
            slot.cancel_all();
        }
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(());
    }

    if let Err(error) = wiki::begin_wiki(app.clone(), request.project_path.clone()) {
        if let Some(watch) = agent_cancel_watch.take() {
            watch.abort();
        }
        if let Some(slot) = &agent_slot {
            slot.cancel_all();
        }
        return fail_wiki_generation(&on_event, &run.token, error);
    }
    for page in &pages {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Page {
                page: page.clone(),
                status: "pending".into(),
                error: None,
                duration_ms: None,
            },
        );
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "generating".into(),
        },
    );

    let page_errors = Arc::new(Mutex::new(Vec::<AppError>::new()));
    match &backend {
        WikiGenerationBackend::Builtin => {
            stream::iter(pages.clone())
                .for_each_concurrent(request.concurrency.clamp(1, 8), |page| {
                    let app = app.clone();
                    let db = &db;
                    let project_path = request.project_path.clone();
                    let language = request.language.clone();
                    let token = run.token.clone();
                    let channel = on_event.clone();
                    let page_errors = page_errors.clone();
                    async move {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page: page.clone(),
                                status: "running".into(),
                                error: None,
                                duration_ms: None,
                            },
                        );
                        let page_started = Instant::now();
                        let progress_channel = channel.clone();
                        let retry_channel = channel.clone();
                        let progress_page_id = page.id.clone();
                        let retry_page_id = page.id.clone();
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::ActivityBatch {
                                activity_type: "read".into(),
                                items: page.relevant_files.clone(),
                            },
                        );
                        let result = generate_builtin_page_to_disk(
                            &app,
                            db,
                            &project_path,
                            &page,
                            &language,
                            &token,
                            move |content| {
                                send_wiki_event(
                                    &progress_channel,
                                    WikiGenerationEvent::Progress {
                                        page_id: progress_page_id.clone(),
                                        content: content.to_string(),
                                    },
                                );
                            },
                            move |notice| {
                                send_wiki_event(
                                    &retry_channel,
                                    WikiGenerationEvent::Retry {
                                        page_id: Some(retry_page_id.clone()),
                                        attempt: notice.attempt,
                                        max_attempts: notice.max_attempts,
                                        delay_seconds: notice.delay_seconds,
                                        reason: notice.reason,
                                    },
                                );
                            },
                        )
                        .await;
                        let (status, error) = if token.is_cancelled() {
                            ("cancelled", None)
                        } else {
                            match result {
                                Ok(()) => ("done", None),
                                Err(error) => {
                                    let message = error.to_string();
                                    page_errors.lock().unwrap().push(error);
                                    ("failed", Some(message))
                                }
                            }
                        };
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page,
                                status: status.into(),
                                error,
                                duration_ms: Some(page_started.elapsed().as_millis() as u64),
                            },
                        );
                    }
                })
                .await;
        }
        WikiGenerationBackend::Agent { .. } => {
            let params = agent_params.as_ref().expect("agent params").clone();
            let slot = agent_slot.as_ref().expect("agent slot").clone();
            // 每页独立会话,互不共享上下文,可以按配置并发(默认 2,上限 8)
            stream::iter(pages.clone())
                .for_each_concurrent(params.concurrency, |page| {
                    let app = app.clone();
                    let db = &db;
                    let params = params.clone();
                    let slot = slot.clone();
                    let token = run.token.clone();
                    let channel = on_event.clone();
                    let language = request.language.clone();
                    let page_errors = page_errors.clone();
                    async move {
                        if token.is_cancelled() {
                            send_wiki_event(
                                &channel,
                                WikiGenerationEvent::Page {
                                    page,
                                    status: "cancelled".into(),
                                    error: None,
                                    duration_ms: None,
                                },
                            );
                            return;
                        }
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page: page.clone(),
                                status: "running".into(),
                                error: None,
                                duration_ms: None,
                            },
                        );
                        let progress_channel = channel.clone();
                        let activity_channel = channel.clone();
                        let retry_channel = channel.clone();
                        let progress_page_id = page.id.clone();
                        let retry_page_id = page.id.clone();
                        let result = generate_agent_page_to_disk(
                            &app,
                            db,
                            &params,
                            &slot,
                            &page,
                            &language,
                            &[],
                            &token,
                            Arc::new(move |content| {
                                send_wiki_event(
                                    &progress_channel,
                                    WikiGenerationEvent::Progress {
                                        page_id: progress_page_id.clone(),
                                        content,
                                    },
                                );
                            }),
                            Arc::new(move |text| {
                                send_wiki_event(
                                    &activity_channel,
                                    WikiGenerationEvent::ActivityBatch {
                                        activity_type: "tool".into(),
                                        items: vec![text],
                                    },
                                );
                            }),
                            Arc::new(move |notice: WikiRetryNotice| {
                                send_wiki_event(
                                    &retry_channel,
                                    WikiGenerationEvent::Retry {
                                        page_id: Some(retry_page_id.clone()),
                                        attempt: notice.attempt,
                                        max_attempts: notice.max_attempts,
                                        delay_seconds: notice.delay_seconds,
                                        reason: notice.reason,
                                    },
                                );
                            }),
                        )
                        .await;
                        let (status, error, duration_ms) = if token.is_cancelled() {
                            ("cancelled", None, None)
                        } else {
                            match result {
                                Ok(stats) => ("done", None, Some(stats.duration_ms)),
                                Err(error) => {
                                    let message = error.to_string();
                                    page_errors.lock().unwrap().push(error);
                                    ("failed", Some(message), None)
                                }
                            }
                        };
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page,
                                status: status.into(),
                                error,
                                duration_ms,
                            },
                        );
                    }
                })
                .await;
        }
    }

    if let Some(watch) = agent_cancel_watch.take() {
        watch.abort();
    }
    if let Some(slot) = &agent_slot {
        slot.cancel_all();
    }
    if run.token.is_cancelled() {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(());
    }
    let page_error = {
        let mut errors = page_errors.lock().unwrap();
        if errors.is_empty() {
            None
        } else {
            Some(errors.remove(0))
        }
    };
    if let Some(error) = page_error {
        return fail_wiki_generation(&on_event, &run.token, error);
    }
    let save_result = wiki::save_wiki_meta(
        app,
        request.project_path.clone(),
        wiki::WikiMeta {
            project_path: request.project_path,
            head_sha: context.head_sha,
            model: meta_model,
            language: request.language,
            status: "completed".into(),
            outline: pages,
            generator: Some(backend_id),
            ..Default::default()
        },
        Some(wiki::WikiCommitKind::Generate),
    );
    if let Err(error) = save_result {
        return fail_wiki_generation(&on_event, &run.token, error);
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "done".into(),
        },
    );
    Ok(())
}

/// 单页/增量 Wiki 生成入口。内置 API 与 ACP 会话生命周期都封装在 Rust。
#[tauri::command]
pub async fn ai_regenerate_wiki_page(
    app: AppHandle,
    db: State<'_, Db>,
    request: RegenerateWikiPageRequest,
    on_progress: Channel<String>,
) -> AppResult<RegeneratedWikiPage> {
    let run = RegisteredRun::new(request.run_id);
    let backend = wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    let page_title = request.page.title.clone();
    let project_path = request.project_path.clone();
    let generated = match backend {
        WikiGenerationBackend::Builtin => {
            generate_builtin_page_to_disk(
                &app,
                &db,
                &request.project_path,
                &request.page,
                &request.language,
                &run.token,
                |content| {
                    let _ = on_progress.send(content.to_string());
                },
                |_| {},
            )
            .await?;
            RegeneratedWikiPage {
                model: sdk::load_config(&app).ai_model,
                generator: "builtin".into(),
            }
        }
        WikiGenerationBackend::Agent { .. } => {
            let params = AgentSessionParams::from_backend(&backend, &request.project_path)
                .expect("agent backend");
            let slot = AgentSessionSlot::default();
            let cancel_watch = watch_agent_cancel(run.token.clone(), slot.clone());
            let progress = Arc::new(move |content: String| {
                let _ = on_progress.send(content);
            });
            let result = generate_agent_page_to_disk(
                &app,
                &db,
                &params,
                &slot,
                &request.page,
                &request.language,
                &request.changed_files,
                &run.token,
                progress,
                Arc::new(|_| {}),
                Arc::new(|_| {}),
            )
            .await;
            slot.cancel_all();
            cancel_watch.abort();
            let stats = result?;
            RegeneratedWikiPage {
                model: stats.usage_model,
                generator: format!("acp:{}", params.agent_id.as_deref().unwrap_or("custom")),
            }
        }
    };
    if let Err(error) = wiki::commit_wiki(
        app,
        project_path,
        wiki::WikiCommitKind::Page,
        Some(page_title),
    ) {
        eprintln!("[wiki] 单页快照提交失败: {error}");
    }
    Ok(generated)
}

/// 增量更新的变更检测、受影响页面筛选、页面生成与 meta 推进全部在后端完成。
/// 生成后端始终从项目 Wiki 目录的 config.json 读取。自动更新不比较旧 Wiki 记录的
/// 生成后端或模型，直接用当前项目配置重生成受影响页面；遇到旧 Wiki 或历史改写时仍
/// 静默跳过。手动更新遇到后端切换等不可增量情况时返回错误，由界面沿既有语义
/// 退化为整本重生成。
#[tauri::command]
pub async fn ai_update_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: UpdateWikiRequest,
    on_event: Channel<WikiUpdateEvent>,
) -> AppResult<usize> {
    let run = RegisteredRun::new(request.run_id);
    let Some(data) = wiki::load_wiki(app.clone(), request.project_path.clone())? else {
        return Ok(0);
    };
    let backend = wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    let Some(from_sha) = data.meta.head_sha.clone() else {
        if request.automatic {
            return Ok(0);
        }
        return Err(AppError::coded(ErrorCode::GitCommandFailed, "no head sha"));
    };
    let backend_id = wiki_backend_id(&backend);
    if should_reject_wiki_backend_change(
        data.meta.generator.as_deref(),
        &backend_id,
        request.automatic,
    ) {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "generator mismatch",
        ));
    }
    let changed = match wiki::wiki_changed_files(request.project_path.clone(), from_sha) {
        Ok(changed) => changed,
        Err(_) if request.automatic => return Ok(0),
        Err(error) => return Err(error),
    };
    let changed_set: HashSet<&str> = changed.files.iter().map(String::as_str).collect();
    let affected: Vec<_> = data
        .meta
        .outline
        .iter()
        .filter(|page| {
            page.relevant_files
                .iter()
                .any(|file| changed_set.contains(file.as_str()))
        })
        .cloned()
        .collect();
    let total = affected.len();
    let _ = on_event.send(WikiUpdateEvent {
        completed: 0,
        total,
    });

    let mut generated_model = data.meta.model.clone();
    let mut generated_generator = data.meta.generator.clone();
    if !affected.is_empty() {
        generated_generator = Some(backend_id.clone());
        match &backend {
            WikiGenerationBackend::Builtin => {
                generated_model = sdk::load_config(&app).ai_model;
                for (index, page) in affected.iter().enumerate() {
                    generate_builtin_page_to_disk(
                        &app,
                        &db,
                        &request.project_path,
                        page,
                        &request.language,
                        &run.token,
                        |_| {},
                        |_| {},
                    )
                    .await?;
                    let _ = on_event.send(WikiUpdateEvent {
                        completed: index + 1,
                        total,
                    });
                }
            }
            WikiGenerationBackend::Agent { .. } => {
                let params = AgentSessionParams::from_backend(&backend, &request.project_path)
                    .expect("agent backend");
                let slot = AgentSessionSlot::default();
                let cancel_watch = watch_agent_cancel(run.token.clone(), slot.clone());
                // 与整本生成同款并发:出错即取消其余页面,最终上报第一个错误
                let first_error = Arc::new(Mutex::new(None::<AppError>));
                let model_cell = Arc::new(Mutex::new(generated_model.clone()));
                let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                stream::iter(affected.clone())
                    .for_each_concurrent(params.concurrency, |page| {
                        let app = app.clone();
                        let db = &db;
                        let params = params.clone();
                        let slot = slot.clone();
                        let token = run.token.clone();
                        let language = request.language.clone();
                        let changed_files = changed.files.clone();
                        let first_error = first_error.clone();
                        let model_cell = model_cell.clone();
                        let completed = completed.clone();
                        let on_event = on_event.clone();
                        async move {
                            if token.is_cancelled() {
                                return;
                            }
                            match generate_agent_page_to_disk(
                                &app,
                                db,
                                &params,
                                &slot,
                                &page,
                                &language,
                                &changed_files,
                                &token,
                                Arc::new(|_| {}),
                                Arc::new(|_| {}),
                                Arc::new(|_| {}),
                            )
                            .await
                            {
                                Ok(stats) => {
                                    *model_cell.lock().unwrap() = stats.usage_model;
                                }
                                Err(error) => {
                                    if !token.is_cancelled() {
                                        *first_error.lock().unwrap() = Some(error);
                                        // 取消其余页面(经 cancel watch 杀进行中会话)
                                        token.cancel();
                                    }
                                }
                            }
                            let done =
                                completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            let _ = on_event.send(WikiUpdateEvent {
                                completed: done,
                                total,
                            });
                        }
                    })
                    .await;
                generated_model = model_cell.lock().unwrap().clone();
                slot.cancel_all();
                cancel_watch.abort();
                let first_error = first_error.lock().unwrap().take();
                if let Some(error) = first_error {
                    return Err(error);
                }
            }
        }
    }

    if let Some(head_sha) = changed.head_sha {
        wiki::save_wiki_meta(
            app,
            request.project_path.clone(),
            wiki::WikiMeta {
                head_sha: Some(head_sha),
                model: generated_model,
                generator: generated_generator,
                ..data.meta
            },
            Some(wiki::WikiCommitKind::Update),
        )?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        sanitize_agent_retry_notices, should_reject_wiki_backend_change, WikiGenerationEvent,
    };

    #[test]
    fn automatic_wiki_update_accepts_backend_change() {
        assert!(!should_reject_wiki_backend_change(
            Some("acp:pi"),
            "acp:opencode",
            true,
        ));
        assert!(should_reject_wiki_backend_change(
            Some("acp:pi"),
            "acp:opencode",
            false,
        ));
    }

    #[test]
    fn wiki_progress_event_uses_frontend_field_names() {
        let value = serde_json::to_value(WikiGenerationEvent::Progress {
            page_id: "overview".into(),
            content: "# Overview".into(),
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "kind": "progress",
                "pageId": "overview",
                "content": "# Overview",
            })
        );

        let activity = serde_json::to_value(WikiGenerationEvent::ActivityBatch {
            activity_type: "read".into(),
            items: vec!["README.md".into()],
        })
        .unwrap();
        assert_eq!(
            activity,
            json!({
                "kind": "activityBatch",
                "activityType": "read",
                "items": ["README.md"],
            })
        );

        let retry = serde_json::to_value(WikiGenerationEvent::Retry {
            page_id: Some("overview".into()),
            attempt: 1,
            max_attempts: 3,
            delay_seconds: 2,
            reason: "rateLimited".into(),
        })
        .unwrap();
        assert_eq!(
            retry,
            json!({
                "kind": "retry",
                "pageId": "overview",
                "attempt": 1,
                "maxAttempts": 3,
                "delaySeconds": 2,
                "reason": "rateLimited",
            })
        );
    }

    #[test]
    fn acp_retry_notices_do_not_enter_wiki_markdown() {
        let (content, retry) = sanitize_agent_retry_notices(
            "Retrying (attempt 1/3, waiting 2s)...Retry finished, resuming.# 请求数据流",
        );
        assert_eq!(content, "# 请求数据流");
        assert_eq!(retry.unwrap().attempt, 1);

        let (partial, retry) =
            sanitize_agent_retry_notices("# 已生成\nRetrying (attempt 2/3, waiting 4s)...");
        assert_eq!(partial, "# 已生成\n");
        assert_eq!(retry.unwrap().delay_seconds, 4);
    }
}
