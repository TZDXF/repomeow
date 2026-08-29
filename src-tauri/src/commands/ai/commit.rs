use std::time::Instant;

use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::ai::prompts::{effective_system_prompt, DEFAULT_COMMIT_PROMPT};
use crate::ai::sdk;
use crate::commands::git::{self, AiCommitFileContext};
use crate::commands::semantic;
use crate::db::Db;
use crate::error::AppResult;

use super::run::record_usage;

/// raw 只在 sem 覆盖缺口或失败时进入模型；仍保留总预算防止极端 diff 撑爆上下文。
const RAW_FALLBACK_MAX_CHARS: usize = 30_000;

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCommitMessageRequest {
    project_path: String,
    project_name: String,
    #[serde(default)]
    project_description: String,
    language: String,
    #[serde(default = "default_true")]
    include_untracked: bool,
    #[serde(default)]
    paths: Option<Vec<String>>,
}

fn file_is_covered(
    file: &AiCommitFileContext,
    covered: &std::collections::HashSet<String>,
) -> bool {
    covered.contains(&file.path)
        || file
            .old_path
            .as_ref()
            .is_some_and(|old_path| covered.contains(old_path))
}

fn raw_fallback(files: &[&AiCommitFileContext]) -> (String, bool) {
    let mut out = String::new();
    let mut truncated = false;
    for file in files {
        let section = if file.binary {
            format!("- {} (binary, status {})\n", file.path, file.status)
        } else if file.raw_excluded {
            format!(
                "- {} (status {}; generated/lockfile content omitted)\n",
                file.path, file.status
            )
        } else {
            file.raw_patch.clone()
        };
        let used = out.chars().count();
        let remaining = RAW_FALLBACK_MAX_CHARS.saturating_sub(used);
        if section.chars().count() > remaining {
            out.extend(section.chars().take(remaining));
            truncated = true;
            break;
        }
        out.push_str(&section);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    (out, truncated)
}

#[tauri::command]
pub async fn ai_generate_commit_message(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateCommitMessageRequest,
) -> AppResult<String> {
    let project_path = request.project_path.clone();
    let context = git::ai_commit_context(
        request.project_path,
        request.include_untracked,
        request.paths,
    )
    .await?;

    // sem 是主数据源。它成功但只覆盖部分文件时仅补缺口；失败/空结果才全量 raw 降级。
    let semantic_result = if context.semantic_input.trim().is_empty() {
        None
    } else {
        semantic::commit_input_analysis(&app, &project_path, &context.semantic_input)
            .await
            .ok()
            .filter(|analysis| !analysis.text.trim().is_empty())
    };
    let (semantic_section, fallback_files, sem_failed) = match &semantic_result {
        Some(analysis) => (
            format!("\n\nSemantic changes (primary source):\n{}", analysis.text),
            context
                .files
                .iter()
                .filter(|file| !file_is_covered(file, &analysis.covered_paths))
                .collect::<Vec<_>>(),
            false,
        ),
        None => (
            String::new(),
            context.files.iter().collect::<Vec<_>>(),
            !context.files.is_empty(),
        ),
    };
    let (raw, raw_truncated) = raw_fallback(&fallback_files);
    let raw_section = if raw.is_empty() {
        String::new()
    } else {
        let title = if sem_failed {
            "Raw diff fallback (semantic analysis unavailable):"
        } else {
            "Raw diff fallback (files not covered by semantic analysis):"
        };
        format!(
            "\n\n{title}{}\n{raw}",
            if raw_truncated {
                "\n(Note: raw fallback was truncated due to length.)"
            } else {
                ""
            }
        )
    };

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
    let user_prompt = format!(
        "{project_section}{recent_section}\n\nChange summary:\n{}{semantic_section}{raw_section}",
        if context.stat.is_empty() {
            "(none)"
        } else {
            &context.stat
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, raw_patch: &str, binary: bool) -> AiCommitFileContext {
        AiCommitFileContext {
            path: path.to_string(),
            old_path: None,
            status: "M".to_string(),
            raw_patch: raw_patch.to_string(),
            binary,
            raw_excluded: false,
        }
    }

    #[test]
    fn raw_fallback_keeps_only_requested_files() {
        let covered = file("covered.rs", "covered raw", false);
        let missing = file("missing.rs", "missing raw", false);
        let (raw, truncated) = raw_fallback(&[&missing]);
        assert!(!raw.contains(&covered.raw_patch));
        assert!(raw.contains(&missing.raw_patch));
        assert!(!truncated);
    }

    #[test]
    fn binary_fallback_never_includes_bytes() {
        let binary = file("asset.bin", "GIT binary patch\nliteral payload", true);
        let (raw, truncated) = raw_fallback(&[&binary]);
        assert_eq!(raw, "- asset.bin (binary, status M)\n");
        assert!(!truncated);
    }
}
