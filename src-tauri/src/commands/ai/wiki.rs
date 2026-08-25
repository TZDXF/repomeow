use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use futures::{stream, StreamExt};
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
mod generation;
mod types;
mod update;

use agent_backend::*;
use builtin_backend::*;
pub use generation::*;
pub use types::*;
pub use update::*;

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
