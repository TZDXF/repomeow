use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::{stream, StreamExt};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::ai::prompts::{
    language_name, AGENT_WIKI_OUTLINE_PROMPT, BUILTIN_AGENT_WIKI_PAGE_PROMPT,
};
use crate::ai::sdk;
use crate::commands::wiki;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};

use super::run::RegisteredRun;

mod builtin_backend;
#[cfg(test)]
mod builtin_backend_tests;
mod generation;
mod types;
mod update;

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

/// 相关文件全文区块:逐行 `N: ` 前缀(行级引用用),大纲与页面 prompt 共用
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

/// 页面 prompt(混合模式):相关文件全文直接喂入,内置 Agent 仅在不足时少量补读
fn builtin_agent_wiki_page_prompt(
    page: &wiki::WikiOutlinePage,
    files: &[wiki::WikiFileContent],
    changed_files: &[String],
    language: &str,
    draft_path: &str,
    has_existing_draft: bool,
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
    let draft_state = if has_existing_draft {
        "The writable draft starts with the current published page. Update it to match the supplied sources and changed files."
    } else {
        "The writable draft is empty. Create the complete page in it."
    };
    format!(
        "{}\n\nWritable draft path (the only path you may modify):\n{}\n\n{}\n\nRespond in {}.\n\nWiki page: {}\nCoverage: {}{}\n\n{}",
        BUILTIN_AGENT_WIKI_PAGE_PROMPT.trim(),
        draft_path,
        draft_state,
        language_name(language),
        page.title,
        page.description,
        changed,
        wiki_files_section(files),
    )
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

fn should_reject_wiki_backend_change(
    previous_backend: Option<&str>,
    current_backend: &str,
    automatic: bool,
) -> bool {
    !automatic && previous_backend.unwrap_or("builtin") != current_backend
}

/// 单页 Wiki 重生成入口。内置 Agent 的 Harness 生命周期封装在 Rust。
#[tauri::command]
pub async fn ai_regenerate_wiki_page(
    app: AppHandle,
    db: State<'_, Db>,
    request: RegenerateWikiPageRequest,
    on_progress: Channel<String>,
) -> AppResult<RegeneratedWikiPage> {
    let run = RegisteredRun::new(request.run_id);
    let config = wiki::load_wiki_config_internal(&app, &request.project_path)?;
    let page_title = request.page.title.clone();
    let project_path = request.project_path.clone();
    let model_name = generate_builtin_page_to_disk(
        &app,
        &db,
        &run.id,
        &request.project_path,
        &request.page,
        &request.language,
        &request.changed_files,
        config.model.as_deref(),
        config.thinking.as_deref(),
        &run.token,
        Arc::new(move |content| {
            let _ = on_progress.send(content);
        }),
        Arc::new(|_| {}),
        Arc::new(|_| {}),
    )
    .await?;
    let generated = RegeneratedWikiPage {
        model: model_name,
        generator: "builtin".into(),
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

    use super::{should_reject_wiki_backend_change, WikiGenerationEvent, WikiUpdateResult};

    #[test]
    fn automatic_wiki_update_accepts_legacy_generator_change() {
        // 旧 Wiki 由已移除的三方 agent 后端生成:自动更新直接用内置重生成,
        // 手动更新拒绝(界面退化为整本重生成)
        assert!(!should_reject_wiki_backend_change(
            Some("acp:pi"),
            "builtin",
            true,
        ));
        assert!(should_reject_wiki_backend_change(
            Some("acp:pi"),
            "builtin",
            false,
        ));
        assert!(!should_reject_wiki_backend_change(
            Some("builtin"),
            "builtin",
            false,
        ));
    }

    #[test]
    fn wiki_update_result_uses_frontend_field_names() {
        let value = serde_json::to_value(WikiUpdateResult {
            updated_page_ids: vec!["overview".into(), "architecture".into()],
        })
        .unwrap();

        assert_eq!(
            value,
            json!({ "updatedPageIds": ["overview", "architecture"] })
        );
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
}
