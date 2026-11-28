use std::time::Instant;

use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::ai::prompts::{effective_system_prompt, language_name, DEFAULT_COMMIT_PROMPT};
use crate::ai::sdk;
use crate::commands::git::{self, AiCommitFileContext};
use crate::db::Db;
use crate::error::AppResult;

use super::run::{record_usage, RegisteredRun};

/// raw diff 进入模型的总预算,防止极端 diff 撑爆上下文。
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
    /// 取消句柄:前端生成独立 runId,取消时经 ai_cancel_run 置位;缺省表示不可取消。
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default = "default_true")]
    include_untracked: bool,
    #[serde(default)]
    paths: Option<Vec<String>>,
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
) -> AppResult<Option<String>> {
    let run = request
        .run_id
        .as_deref()
        .map(|id| RegisteredRun::new(id.to_string()));
    let context = git::ai_commit_context(
        request.project_path,
        request.include_untracked,
        request.paths,
    )
    .await?;

    let files = context.files.iter().collect::<Vec<_>>();
    let (raw, raw_truncated) = raw_fallback(&files);
    let raw_section = if raw.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nChange diff:{}\n{raw}",
            if raw_truncated {
                "\n(Note: diff was truncated due to length.)"
            } else {
                ""
            }
        )
    };

    let description = request.project_description.trim();
    let mut project_section = format!("Project: {}", request.project_name);
    if !description.is_empty() {
        project_section.push_str(&format!("\nDescription: {description}"));
    }
    if !context.branch.is_empty() {
        project_section.push_str(&format!("\nBranch: {}", context.branch));
    }
    let user_prompt = format!(
        "{project_section}\n\nChange summary:\n{}{raw_section}\n\nWrite the commit message in {}.",
        if context.stat.is_empty() {
            "(none)"
        } else {
            &context.stat
        },
        language_name(&request.language),
    );
    let system_prompt =
        effective_system_prompt(&app, "commit.md", DEFAULT_COMMIT_PROMPT, &request.language);
    let config = sdk::load_config_for(&app, "commit");
    let budget = crate::ai::budget::RequestBudget::new(&config.model(), &system_prompt, "", None)?;
    let user_prompt = if budget.fits(&user_prompt) {
        user_prompt
    } else {
        let marker = format!(
            "\n[部分变更因模型上下文预算省略]\nWrite the commit message in {}.",
            language_name(&request.language)
        );
        let chunks = budget.split(&user_prompt, budget.input - budget.tokens(&marker) - 32)?;
        format!("{}{marker}", chunks.first().copied().unwrap_or_default())
    };
    let started = Instant::now();
    let output = match sdk::chat(
        &config,
        Some(&system_prompt),
        &user_prompt,
        false,
        None,
        run.as_ref().map(|run| &run.token),
    )
    .await
    {
        Ok(output) => output,
        // 取消不算错误:与 report 一致返回 None,前端静默收场
        Err(_) if run.as_ref().is_some_and(|run| run.token.is_cancelled()) => return Ok(None),
        Err(error) => return Err(error),
    };
    record_usage(
        &db,
        "commit",
        &config.ai_model,
        &output,
        started.elapsed().as_millis() as i64,
    );
    // 模型偶尔无视提示词仍用 ``` 围栏包裹提交信息,返回前兜底剥离
    Ok(Some(sdk::strip_code_fence(&output.text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, raw_patch: &str, binary: bool) -> AiCommitFileContext {
        AiCommitFileContext {
            path: path.to_string(),
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
    fn lockfile_content_is_omitted_but_still_listed() {
        let code = file("src/main.rs", "code raw", false);
        let lock = AiCommitFileContext {
            raw_excluded: true,
            ..file("Cargo.lock", "lock raw", false)
        };
        let (raw, truncated) = raw_fallback(&[&code, &lock]);
        assert!(raw.contains("code raw"));
        assert!(raw.contains("- Cargo.lock (status M; generated/lockfile content omitted)"));
        assert!(!raw.contains("lock raw"));
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
