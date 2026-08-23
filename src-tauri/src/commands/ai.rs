use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::ai::prompts::{
    effective_system_prompt, fixed_system_prompt, language_name, AGENT_WIKI_OUTLINE_PROMPT,
    AGENT_WIKI_PAGE_PROMPT, DEFAULT_COMMIT_PROMPT, DEFAULT_REPORT_PROMPT,
    DEFAULT_WEEKLY_REPORT_PROMPT, DEFAULT_WIKI_OUTLINE_PROMPT, DEFAULT_WIKI_PAGE_PROMPT,
};
use crate::ai::sdk::{self, AiConfig, ChatOutput};
use crate::commands::usage::insert_usage_row;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{AiUsageRecord, GitCommitInfo};
use crate::time_util::now_ts;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateCommitMessageRequest {
    project_path: String,
    project_name: String,
    #[serde(default)]
    project_description: String,
    language: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportProjectCommits {
    #[serde(default)]
    project_id: Option<i64>,
    project_name: String,
    #[serde(default)]
    project_description: String,
    commits: Vec<GitCommitInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateAndSaveReportRequest {
    run_id: String,
    project_ids: Vec<i64>,
    date_from: String,
    date_to: String,
    range_label: String,
    author_mode: String,
    language: String,
    period_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedReport {
    history_id: i64,
    result: String,
    commit_data: Vec<ReportProjectCommits>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReportItem {
    date_from: String,
    date_to: String,
    label: String,
    status: String,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBatchReportsRequest {
    run_id: String,
    items: Vec<BatchReportItem>,
    project_ids: Vec<i64>,
    author_mode: String,
    language: String,
    period_type: String,
    concurrency: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchReportEvent {
    date_from: String,
    date_to: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

static AI_RUNS: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();

fn ai_runs() -> &'static Mutex<HashMap<String, CancellationToken>> {
    AI_RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct RegisteredRun {
    id: String,
    token: CancellationToken,
}

impl RegisteredRun {
    fn new(id: String) -> Self {
        let token = CancellationToken::new();
        ai_runs().lock().unwrap().insert(id.clone(), token.clone());
        Self { id, token }
    }
}

impl Drop for RegisteredRun {
    fn drop(&mut self) {
        ai_runs().lock().unwrap().remove(&self.id);
    }
}

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
    page: super::wiki::WikiOutlinePage,
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
        page: super::wiki::WikiOutlinePage,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Progress {
        page_id: String,
        content: String,
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

fn record_usage(db: &Db, task_type: &str, model: &str, output: &ChatOutput, duration_ms: i64) {
    let usage = output.usage.as_ref();
    let record = AiUsageRecord {
        task_type: task_type.to_string(),
        model: model.to_string(),
        input_tokens: usage.and_then(|value| value.input_tokens),
        output_tokens: usage.and_then(|value| value.output_tokens),
        total_tokens: usage.and_then(|value| value.total_tokens),
        duration_ms: Some(duration_ms),
        cached_tokens: usage.and_then(|value| value.cached_tokens),
    };
    if let Ok(conn) = db.0.lock() {
        let _ = insert_usage_row(&conn, &record, now_ts());
    }
}

#[tauri::command]
pub async fn ai_list_models(config: AiConfig) -> AppResult<Vec<String>> {
    sdk::list_models(&config.normalized()).await
}

#[tauri::command]
pub async fn ai_test_connection(app: AppHandle) -> AppResult<()> {
    let config = sdk::load_config(&app);
    sdk::chat(
        &config,
        None,
        "Reply with the single word: ok",
        false,
        Some(8),
        None,
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn ai_generate_commit_message(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateCommitMessageRequest,
) -> AppResult<String> {
    let context = super::git::git_commit_context(request.project_path).await?;
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

async fn generate_report_text(
    app: &AppHandle,
    db: &Db,
    data: &[ReportProjectCommits],
    range_label: &str,
    language: &str,
    period_type: &str,
    cancel: &CancellationToken,
) -> AppResult<String> {
    let sections = data
        .iter()
        .map(|project| {
            let description = project.project_description.trim();
            let heading = if description.is_empty() {
                project.project_name.clone()
            } else {
                format!("{} — {description}", project.project_name)
            };
            let commits = project
                .commits
                .iter()
                .map(|commit| {
                    format!(
                        "- [{}] {} ({}, {})",
                        commit.date, commit.subject, commit.hash, commit.author
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("### {heading}\n{commits}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let weekly = period_type == "weekly";
    let system_prompt = effective_system_prompt(
        app,
        if weekly {
            "report-weekly.md"
        } else {
            "report.md"
        },
        if weekly {
            DEFAULT_WEEKLY_REPORT_PROMPT
        } else {
            DEFAULT_REPORT_PROMPT
        },
        language,
    );
    let user_prompt = format!(
        "Time range: {}.\n\nCommit records:\n{sections}",
        range_label
    );
    let config = sdk::load_config(app);
    let started = Instant::now();
    let output = sdk::chat(
        &config,
        Some(&system_prompt),
        &user_prompt,
        false,
        None,
        Some(cancel),
    )
    .await?;
    record_usage(
        db,
        "report",
        &config.ai_model,
        &output,
        started.elapsed().as_millis() as i64,
    );
    Ok(output.text)
}

#[derive(Clone)]
struct ReportProject {
    id: i64,
    path: String,
    name: String,
    description: String,
}

fn load_report_projects(db: &Db, project_ids: &[i64]) -> AppResult<Vec<ReportProject>> {
    let selected: HashSet<i64> = project_ids.iter().copied().collect();
    let conn = db.0.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, path, name, description FROM projects WHERE archived_at IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ReportProject {
            id: row.get(0)?,
            path: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
        })
    })?;
    Ok(rows
        .filter_map(Result::ok)
        .filter(|project| selected.contains(&project.id))
        .collect())
}

async fn collect_report_commits(
    db: &Db,
    project_ids: &[i64],
    date_from: &str,
    date_to: &str,
    author_mode: &str,
) -> AppResult<Vec<ReportProjectCommits>> {
    let projects = load_report_projects(db, project_ids)?;
    let since = format!("{date_from} 00:00:00");
    let until = format!("{date_to} 23:59:59");
    let mine_only = author_mode == "me";
    tokio::task::spawn_blocking(move || {
        projects
            .into_iter()
            .map(|project| {
                let author = if mine_only {
                    super::git::run_git_current_user(&project.path)
                        .ok()
                        .and_then(|user| {
                            let name = user.name.trim();
                            let email = user.email.trim();
                            if !name.is_empty() {
                                Some(name.to_string())
                            } else if !email.is_empty() {
                                Some(email.to_string())
                            } else {
                                None
                            }
                        })
                } else {
                    None
                };
                let commits = super::git::run_git_log(
                    &project.path,
                    Some(&since),
                    Some(&until),
                    Some(500),
                    author.as_deref(),
                )?;
                Ok(ReportProjectCommits {
                    project_id: Some(project.id),
                    project_name: project.name,
                    project_description: project.description,
                    commits,
                })
            })
            .collect::<AppResult<Vec<_>>>()
    })
    .await
    .map_err(|error| AppError::coded(ErrorCode::ReportTaskFailed, error.to_string()))?
}

fn save_generated_report(
    app: &AppHandle,
    db: &Db,
    request: &GenerateAndSaveReportRequest,
    result: String,
    data: Vec<ReportProjectCommits>,
) -> AppResult<GeneratedReport> {
    let commit_data: Vec<super::report::SaveReportCommit> = data
        .iter()
        .map(|project| super::report::SaveReportCommit {
            project_id: project.project_id,
            project_name: project.project_name.clone(),
            project_description: project.project_description.clone(),
            commits: project.commits.clone(),
        })
        .collect();
    let project_ids: Vec<i64> = commit_data
        .iter()
        .filter_map(|project| project.project_id)
        .collect();
    let conn = db.0.lock().unwrap();
    let history_id = super::report::save_report_history_impl(
        app,
        &conn,
        &project_ids,
        &request.date_from,
        &request.date_to,
        &request.range_label,
        &request.author_mode,
        &request.language,
        &request.period_type,
        &result,
        &commit_data,
    )?;
    Ok(GeneratedReport {
        history_id,
        result,
        commit_data: data,
    })
}

/// 手动报告的完整后端管线：读取项目与 Git 提交、生成正文并保存历史。
#[tauri::command]
pub async fn ai_generate_and_save_report(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateAndSaveReportRequest,
) -> AppResult<Option<GeneratedReport>> {
    let run = RegisteredRun::new(request.run_id.clone());
    let data = collect_report_commits(
        &db,
        &request.project_ids,
        &request.date_from,
        &request.date_to,
        &request.author_mode,
    )
    .await?
    .into_iter()
    .filter(|project| !project.commits.is_empty())
    .collect::<Vec<_>>();
    if data.is_empty() || run.token.is_cancelled() {
        return Ok(None);
    }
    let result = generate_report_text(
        &app,
        &db,
        &data,
        &request.range_label,
        &request.language,
        &request.period_type,
        &run.token,
    )
    .await?;
    if run.token.is_cancelled() {
        return Ok(None);
    }
    save_generated_report(&app, &db, &request, result, data).map(Some)
}

fn send_batch_status(
    channel: &Channel<BatchReportEvent>,
    item: &BatchReportItem,
    status: &str,
    error: Option<String>,
) {
    let _ = channel.send(BatchReportEvent {
        date_from: item.date_from.clone(),
        date_to: item.date_to.clone(),
        status: status.to_string(),
        error,
    });
}

/// 批量报告在 Rust 中并发执行；Channel 只向前端投递可渲染的状态变化。
#[tauri::command]
pub async fn ai_generate_batch_reports(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateBatchReportsRequest,
    on_event: Channel<BatchReportEvent>,
) -> AppResult<()> {
    let run = RegisteredRun::new(request.run_id.clone());
    let concurrency = request.concurrency.clamp(1, 8);
    let pending: Vec<BatchReportItem> = request
        .items
        .iter()
        .filter(|item| item.status == "pending")
        .cloned()
        .collect();
    let project_ids = request.project_ids.clone();
    let author_mode = request.author_mode.clone();
    let language = request.language.clone();
    let period_type = request.period_type.clone();

    stream::iter(pending)
        .for_each_concurrent(concurrency, |item| {
            let app = app.clone();
            let db = &db;
            let token = run.token.clone();
            let project_ids = project_ids.clone();
            let author_mode = author_mode.clone();
            let language = language.clone();
            let period_type = period_type.clone();
            let on_event = on_event.clone();
            async move {
                if token.is_cancelled() {
                    send_batch_status(&on_event, &item, "cancelled", None);
                    return;
                }
                send_batch_status(&on_event, &item, "running", None);
                let outcome = async {
                    let data = collect_report_commits(
                        db,
                        &project_ids,
                        &item.date_from,
                        &item.date_to,
                        &author_mode,
                    )
                    .await?
                    .into_iter()
                    .filter(|project| !project.commits.is_empty())
                    .collect::<Vec<_>>();
                    if data.is_empty() {
                        return Ok::<_, AppError>(false);
                    }
                    let result = generate_report_text(
                        &app,
                        db,
                        &data,
                        &item.label,
                        &language,
                        &period_type,
                        &token,
                    )
                    .await?;
                    if token.is_cancelled() {
                        return Ok(false);
                    }
                    let single = GenerateAndSaveReportRequest {
                        run_id: String::new(),
                        project_ids: project_ids.clone(),
                        date_from: item.date_from.clone(),
                        date_to: item.date_to.clone(),
                        range_label: item.label.clone(),
                        author_mode: author_mode.clone(),
                        language: language.clone(),
                        period_type: period_type.clone(),
                    };
                    save_generated_report(&app, db, &single, result, data)?;
                    Ok(true)
                }
                .await;
                match outcome {
                    Ok(true) => send_batch_status(&on_event, &item, "done", None),
                    Ok(false) if token.is_cancelled() => {
                        send_batch_status(&on_event, &item, "cancelled", None)
                    }
                    Ok(false) => send_batch_status(&on_event, &item, "skipped-no-commits", None),
                    Err(error) if token.is_cancelled() => {
                        send_batch_status(&on_event, &item, "cancelled", None)
                    }
                    Err(error) => {
                        send_batch_status(&on_event, &item, "failed", Some(error.to_string()))
                    }
                }
            }
        })
        .await;
    Ok(())
}

fn wiki_outline_user_prompt(context: &super::wiki::WikiContext, project_name: &str) -> String {
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

fn wiki_page_user_prompt(
    page: &super::wiki::WikiOutlinePage,
    files: &[super::wiki::WikiFileContent],
) -> String {
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
        "Wiki page: {}\nCoverage: {}\n\nSource files:\n{}",
        page.title,
        page.description,
        if files_section.is_empty() {
            "(no source files available)"
        } else {
            &files_section
        }
    )
}

fn agent_wiki_outline_prompt(
    context: &super::wiki::WikiContext,
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

fn agent_wiki_page_prompt(
    page: &super::wiki::WikiOutlinePage,
    changed_files: &[String],
    language: &str,
) -> String {
    let relevant = if page.relevant_files.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nSuggested source files (verify they still exist):\n{}",
            page.relevant_files
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
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
        "{}\n\nRespond in {}.\n\nWiki page: {}\nCoverage: {}{}{}",
        AGENT_WIKI_PAGE_PROMPT.trim(),
        language_name(language),
        page.title,
        page.description,
        relevant,
        changed,
    )
}

fn record_acp_usage(
    db: &Db,
    model: &str,
    result: &super::agent::AcpPromptResult,
    duration_ms: i64,
) {
    let Some(usage) = result.usage else { return };
    let record = AiUsageRecord {
        task_type: "wiki".into(),
        model: model.to_string(),
        input_tokens: i64::try_from(usage.input_tokens).ok(),
        output_tokens: i64::try_from(usage.output_tokens).ok(),
        total_tokens: i64::try_from(usage.total_tokens).ok(),
        duration_ms: Some(duration_ms),
        cached_tokens: usage
            .cached_read_tokens
            .and_then(|value| i64::try_from(value).ok()),
    };
    if let Ok(conn) = db.0.lock() {
        let _ = insert_usage_row(&conn, &record, now_ts());
    }
}

#[tauri::command]
pub fn ai_cancel_run(run_id: String) -> AppResult<()> {
    if let Some(token) = ai_runs().lock().unwrap().get(&run_id) {
        token.cancel();
    }
    Ok(())
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

async fn generate_builtin_outline_pages(
    app: &AppHandle,
    db: &Db,
    context: &super::wiki::WikiContext,
    project_name: &str,
    language: &str,
    cancel: &CancellationToken,
) -> AppResult<Vec<super::wiki::WikiOutlinePage>> {
    let config = sdk::load_config(app);
    let system = fixed_system_prompt(DEFAULT_WIKI_OUTLINE_PROMPT, language);
    let original_prompt = wiki_outline_user_prompt(context, project_name);
    let mut prompt = original_prompt.clone();
    let valid_files: HashSet<String> = context.paths.iter().cloned().collect();
    let mut last_error = "wiki outline JSON was not generated".to_string();
    const MAX_ATTEMPTS: usize = 3;
    for _ in 1..=MAX_ATTEMPTS {
        let started = Instant::now();
        let output = sdk::stream_chat(&config, &system, &prompt, true, cancel, |_| {}).await?;
        record_usage(
            db,
            "wiki",
            &config.ai_model,
            &output,
            started.elapsed().as_millis() as i64,
        );
        match crate::ai::wiki_outline::parse_outline(&output.text, &valid_files) {
            Ok(pages) => return Ok(pages),
            Err(error) => {
                last_error = error;
                prompt = outline_retry_prompt(&original_prompt, &last_error);
            }
        }
    }
    Err(AppError::coded(
        ErrorCode::AiResponseParseFailed,
        last_error,
    ))
}

async fn generate_builtin_page_to_disk(
    app: &AppHandle,
    db: &Db,
    project_path: &str,
    page: &super::wiki::WikiOutlinePage,
    language: &str,
    cancel: &CancellationToken,
    on_progress: impl Fn(&str),
) -> AppResult<()> {
    let config = sdk::load_config(app);
    let system = fixed_system_prompt(DEFAULT_WIKI_PAGE_PROMPT, language);
    let files = super::wiki::read_wiki_files_in(project_path, &page.relevant_files)?;
    let prompt = wiki_page_user_prompt(page, &files);
    let mut last_error = None;
    for _ in 0..=2 {
        on_progress("");
        let started = Instant::now();
        match sdk::stream_chat(&config, &system, &prompt, true, cancel, |text| {
            on_progress(text)
        })
        .await
        {
            Ok(output) => {
                record_usage(
                    db,
                    "wiki",
                    &config.ai_model,
                    &output,
                    started.elapsed().as_millis() as i64,
                );
                super::wiki::save_wiki_page_internal(app, project_path, &page.file, &output.text)?;
                return Ok(());
            }
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::coded(ErrorCode::AiRequestFailed, "wiki page generation failed")
    }))
}

async fn generate_agent_outline_pages(
    db: &Db,
    run_id: &str,
    usage_model: &str,
    context: &super::wiki::WikiContext,
    project_name: &str,
    language: &str,
    on_activity: Arc<dyn Fn(String) + Send + Sync>,
) -> AppResult<Vec<super::wiki::WikiOutlinePage>> {
    let original_prompt = agent_wiki_outline_prompt(context, project_name, language);
    let mut prompt = original_prompt.clone();
    let valid_files: HashSet<String> = context.paths.iter().cloned().collect();
    let mut last_error = "wiki outline JSON was not generated".to_string();
    const MAX_ATTEMPTS: usize = 3;
    for _ in 1..=MAX_ATTEMPTS {
        let started = Instant::now();
        let activity = on_activity.clone();
        let result = super::agent::acp_prompt_with(
            run_id.to_string(),
            prompt.clone(),
            super::agent::AcpEventSender::new(move |event| match event {
                super::agent::AcpEvent::Chunk { .. } => {}
                super::agent::AcpEvent::Activity { text } => activity(text),
            }),
        )
        .await?;
        record_acp_usage(
            db,
            usage_model,
            &result,
            started.elapsed().as_millis() as i64,
        );
        let outline = sdk::strip_thinking(&result.text);
        match crate::ai::wiki_outline::parse_outline(&outline, &valid_files) {
            Ok(pages) => return Ok(pages),
            Err(error) => {
                last_error = error;
                prompt = outline_retry_prompt(&original_prompt, &last_error);
            }
        }
    }
    Err(AppError::coded(
        ErrorCode::AiResponseParseFailed,
        last_error,
    ))
}

async fn generate_agent_page_to_disk(
    app: &AppHandle,
    db: &Db,
    run_id: &str,
    usage_model: &str,
    project_path: &str,
    page: &super::wiki::WikiOutlinePage,
    language: &str,
    changed_files: &[String],
    on_progress: Arc<dyn Fn(String) + Send + Sync>,
    on_activity: Arc<dyn Fn(String) + Send + Sync>,
) -> AppResult<()> {
    let prompt = agent_wiki_page_prompt(page, changed_files, language);
    let mut last_error = None;
    for _ in 0..=2 {
        on_progress(String::new());
        let progress = on_progress.clone();
        let activity = on_activity.clone();
        let sender = super::agent::AcpEventSender::new(move |event| match event {
            super::agent::AcpEvent::Chunk { text } => {
                progress(sdk::strip_thinking(&text));
            }
            super::agent::AcpEvent::Activity { text } => activity(text),
        });
        let started = Instant::now();
        match super::agent::acp_prompt_with(run_id.to_string(), prompt.clone(), sender).await {
            Ok(result) => {
                record_acp_usage(
                    db,
                    usage_model,
                    &result,
                    started.elapsed().as_millis() as i64,
                );
                let page_content = sdk::strip_thinking(&result.text);
                if page_content.is_empty() {
                    last_error = Some(AppError::coded(
                        ErrorCode::AgentPromptFailed,
                        "wiki page response is empty after removing thinking content",
                    ));
                    continue;
                }
                super::wiki::save_wiki_page_internal(app, project_path, &page.file, &page_content)?;
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::coded(ErrorCode::AgentPromptFailed, "wiki page generation failed")
    }))
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
    let backend = super::wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "collecting".into(),
        },
    );
    let context = match super::wiki::collect_wiki_context(request.project_path.clone()) {
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
    let mut agent_run_id = None;
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
            )
            .await
        }
        WikiGenerationBackend::Agent {
            agent_id,
            custom_command,
            model,
            thinking,
        } => {
            let started = match super::agent::acp_start(
                agent_id.clone(),
                custom_command.clone(),
                request.project_path.clone(),
                model.clone(),
                thinking.clone(),
            )
            .await
            {
                Ok(started) => started,
                Err(error) => return fail_wiki_generation(&on_event, &run.token, error),
            };
            backend_id = format!("acp:{}", agent_id.as_deref().unwrap_or("custom"));
            meta_model = model
                .as_ref()
                .map(|model| format!("{} · {model}", started.agent_name))
                .unwrap_or_else(|| started.agent_name.clone());
            agent_run_id = Some(started.run_id.clone());
            let token = run.token.clone();
            let cancel_id = started.run_id.clone();
            agent_cancel_watch = Some(tauri::async_runtime::spawn(async move {
                token.cancelled().await;
                let _ = super::agent::acp_cancel(cancel_id);
            }));
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: "outlining".into(),
                },
            );
            generate_agent_outline_pages(
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
            )
            .await
        }
    };

    let pages = match pages_result {
        Ok(pages) => pages,
        Err(error) => {
            if let Some(watch) = agent_cancel_watch.take() {
                watch.abort();
            }
            if let Some(id) = agent_run_id {
                let _ = super::agent::acp_cancel(id);
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
        if let Some(id) = agent_run_id.take() {
            let _ = super::agent::acp_cancel(id);
        }
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(());
    }

    if let Err(error) = super::wiki::begin_wiki(app.clone(), request.project_path.clone()) {
        if let Some(watch) = agent_cancel_watch.take() {
            watch.abort();
        }
        if let Some(id) = agent_run_id.take() {
            let _ = super::agent::acp_cancel(id);
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
            },
        );
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "generating".into(),
        },
    );

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
                    async move {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page: page.clone(),
                                status: "running".into(),
                                error: None,
                            },
                        );
                        let progress_channel = channel.clone();
                        let page_id = page.id.clone();
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
                                        page_id: page_id.clone(),
                                        content: content.to_string(),
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
                                Err(error) => ("failed", Some(error.to_string())),
                            }
                        };
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page,
                                status: status.into(),
                                error,
                            },
                        );
                    }
                })
                .await;
        }
        WikiGenerationBackend::Agent { .. } => {
            let id = agent_run_id.as_ref().expect("agent run id").clone();
            for page in pages.clone() {
                if run.token.is_cancelled() {
                    send_wiki_event(
                        &on_event,
                        WikiGenerationEvent::Page {
                            page,
                            status: "cancelled".into(),
                            error: None,
                        },
                    );
                    continue;
                }
                send_wiki_event(
                    &on_event,
                    WikiGenerationEvent::Page {
                        page: page.clone(),
                        status: "running".into(),
                        error: None,
                    },
                );
                let progress_channel = on_event.clone();
                let activity_channel = on_event.clone();
                let page_id = page.id.clone();
                let result = generate_agent_page_to_disk(
                    &app,
                    &db,
                    &id,
                    &meta_model,
                    &request.project_path,
                    &page,
                    &request.language,
                    &[],
                    Arc::new(move |content| {
                        send_wiki_event(
                            &progress_channel,
                            WikiGenerationEvent::Progress {
                                page_id: page_id.clone(),
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
                )
                .await;
                let (status, error) = if run.token.is_cancelled() {
                    ("cancelled", None)
                } else {
                    match result {
                        Ok(()) => ("done", None),
                        Err(error) => ("failed", Some(error.to_string())),
                    }
                };
                send_wiki_event(
                    &on_event,
                    WikiGenerationEvent::Page {
                        page,
                        status: status.into(),
                        error,
                    },
                );
            }
        }
    }

    if let Some(id) = agent_run_id {
        let _ = super::agent::acp_cancel(id);
    }
    if let Some(watch) = agent_cancel_watch.take() {
        watch.abort();
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
    let save_result = super::wiki::save_wiki_meta(
        app,
        request.project_path.clone(),
        super::wiki::WikiMeta {
            project_path: request.project_path,
            head_sha: context.head_sha,
            model: meta_model,
            language: request.language,
            status: "completed".into(),
            outline: pages,
            generator: Some(backend_id),
            ..Default::default()
        },
        Some(super::wiki::WikiCommitKind::Generate),
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
    let backend = super::wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
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
            )
            .await?;
            RegeneratedWikiPage {
                model: sdk::load_config(&app).ai_model,
                generator: "builtin".into(),
            }
        }
        WikiGenerationBackend::Agent {
            agent_id,
            custom_command,
            model,
            thinking,
        } => {
            let started = super::agent::acp_start(
                agent_id.clone(),
                custom_command,
                request.project_path.clone(),
                model.clone(),
                thinking,
            )
            .await?;
            let usage_model = model
                .as_ref()
                .map(|model| format!("{} · {model}", started.agent_name))
                .unwrap_or_else(|| started.agent_name.clone());
            let token = run.token.clone();
            let cancel_id = started.run_id.clone();
            let cancel_watch = tauri::async_runtime::spawn(async move {
                token.cancelled().await;
                let _ = super::agent::acp_cancel(cancel_id);
            });
            let progress = Arc::new(move |content: String| {
                let _ = on_progress.send(content);
            });
            let result = generate_agent_page_to_disk(
                &app,
                &db,
                &started.run_id,
                &usage_model,
                &request.project_path,
                &request.page,
                &request.language,
                &request.changed_files,
                progress,
                Arc::new(|_| {}),
            )
            .await;
            let _ = super::agent::acp_cancel(started.run_id);
            cancel_watch.abort();
            result?;
            RegeneratedWikiPage {
                model: usage_model,
                generator: format!("acp:{}", agent_id.as_deref().unwrap_or("custom")),
            }
        }
    };
    if let Err(error) = super::wiki::commit_wiki(
        app,
        project_path,
        super::wiki::WikiCommitKind::Page,
        Some(page_title),
    ) {
        eprintln!("[wiki] 单页快照提交失败: {error}");
    }
    Ok(generated)
}

/// 增量更新的变更检测、受影响页面筛选、页面生成与 meta 推进全部在后端完成。
/// 生成后端始终从项目 Wiki 目录的 config.json 读取。自动更新遇到旧 wiki、历史
/// 改写或后端切换时静默跳过；手动更新对不可增量的情况返回错误，由界面沿既有
/// 语义退化为整本重生成。
#[tauri::command]
pub async fn ai_update_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: UpdateWikiRequest,
    on_event: Channel<WikiUpdateEvent>,
) -> AppResult<usize> {
    let run = RegisteredRun::new(request.run_id);
    let Some(data) = super::wiki::load_wiki(app.clone(), request.project_path.clone())? else {
        return Ok(0);
    };
    let backend = super::wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    let Some(from_sha) = data.meta.head_sha.clone() else {
        if request.automatic {
            return Ok(0);
        }
        return Err(AppError::coded(ErrorCode::GitCommandFailed, "no head sha"));
    };
    let backend_id = wiki_backend_id(&backend);
    if data.meta.generator.as_deref().unwrap_or("builtin") != backend_id {
        if request.automatic {
            return Ok(0);
        }
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "generator mismatch",
        ));
    }
    let changed = match super::wiki::wiki_changed_files(request.project_path.clone(), from_sha) {
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
                    )
                    .await?;
                    let _ = on_event.send(WikiUpdateEvent {
                        completed: index + 1,
                        total,
                    });
                }
            }
            WikiGenerationBackend::Agent {
                agent_id,
                custom_command,
                model,
                thinking,
            } => {
                let started = super::agent::acp_start(
                    agent_id.clone(),
                    custom_command.clone(),
                    request.project_path.clone(),
                    model.clone(),
                    thinking.clone(),
                )
                .await?;
                generated_model = model
                    .as_ref()
                    .map(|model| format!("{} · {model}", started.agent_name))
                    .unwrap_or_else(|| started.agent_name.clone());
                let token = run.token.clone();
                let cancel_id = started.run_id.clone();
                let cancel_watch = tauri::async_runtime::spawn(async move {
                    token.cancelled().await;
                    let _ = super::agent::acp_cancel(cancel_id);
                });
                let outcome = async {
                    for (index, page) in affected.iter().enumerate() {
                        generate_agent_page_to_disk(
                            &app,
                            &db,
                            &started.run_id,
                            &generated_model,
                            &request.project_path,
                            page,
                            &request.language,
                            &changed.files,
                            Arc::new(|_| {}),
                            Arc::new(|_| {}),
                        )
                        .await?;
                        let _ = on_event.send(WikiUpdateEvent {
                            completed: index + 1,
                            total,
                        });
                    }
                    Ok::<(), AppError>(())
                }
                .await;
                let _ = super::agent::acp_cancel(started.run_id);
                cancel_watch.abort();
                outcome?;
            }
        }
    }

    if let Some(head_sha) = changed.head_sha {
        super::wiki::save_wiki_meta(
            app,
            request.project_path.clone(),
            super::wiki::WikiMeta {
                head_sha: Some(head_sha),
                model: generated_model,
                generator: generated_generator,
                ..data.meta
            },
            Some(super::wiki::WikiCommitKind::Update),
        )?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::WikiGenerationEvent;

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
    }
}
