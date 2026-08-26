use std::collections::HashSet;
use std::time::Instant;

use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::ai::prompts::{effective_system_prompt, DEFAULT_COMMIT_PROMPT};
use crate::ai::sdk;
use crate::commands::git;
use crate::db::Db;
use crate::error::AppResult;

use super::run::record_usage;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCommitMessageRequest {
    project_path: String,
    project_name: String,
    #[serde(default)]
    project_description: String,
    language: String,
}

#[tauri::command]
pub async fn ai_generate_commit_message(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateCommitMessageRequest,
) -> AppResult<String> {
    let context = git::git_commit_context(request.project_path).await?;
    let description = request.project_description.trim();
    let project_section = if description.is_empty() {
        format!("Project: {}", request.project_name)
    } else {
        format!(
            "Project: {}\nDescription: {description}",
            request.project_name
        )
    };
    let recent_section = if context.recent_commits.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRecent commit messages (match their style and language):\n{}",
            context
                .recent_commits
                .iter()
                .map(|message| format!("- {message}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let truncated_note = if context.truncated {
        "\n(Note: the diff was truncated due to length.)"
    } else {
        ""
    };
    let with_content: HashSet<&str> = context
        .untracked_files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    let names_only: Vec<&str> = context
        .untracked
        .iter()
        .map(String::as_str)
        .filter(|path| !with_content.contains(path))
        .collect();
    let untracked_names = if names_only.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nUntracked new files (no diff content available):\n{}",
            names_only.join("\n")
        )
    };
    let untracked_contents = if context.untracked_files.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nNew file contents (untracked):\n{}",
            context
                .untracked_files
                .iter()
                .map(|file| format!(
                    "=== {}{} ===\n{}",
                    file.path,
                    if file.truncated { " (truncated)" } else { "" },
                    file.content
                ))
                .collect::<Vec<_>>()
                .join("\n\n")
        )
    };
    let user_prompt = format!(
        "{project_section}{recent_section}\n\nChange summary (git diff --stat):\n{}\n\nDiff:{truncated_note}\n{}{}{}",
        if context.stat.is_empty() { "(none)" } else { &context.stat },
        if context.diff.is_empty() { "(empty)" } else { &context.diff },
        untracked_names,
        untracked_contents,
    );
    let system_prompt =
        effective_system_prompt(&app, "commit.md", DEFAULT_COMMIT_PROMPT, &request.language);
    let config = sdk::load_config(&app);
    let started = Instant::now();
    let output = sdk::chat(
        &config,
        Some(&system_prompt),
        &user_prompt,
        false,
        None,
        None,
    )
    .await?;
    record_usage(
        &db,
        "commit",
        &config.ai_model,
        &output,
        started.elapsed().as_millis() as i64,
    );
    Ok(output.text)
}
