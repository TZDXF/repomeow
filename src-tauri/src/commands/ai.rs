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

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiUpdateResult {
    updated_page_ids: Vec<String>,
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

/// 相关文件全文区块:逐行 `N: ` 前缀(行级引用用),内置与 agent 后端的页面 prompt 共用
fn wiki_files_section(files: &[super::wiki::WikiFileContent]) -> String {
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

fn wiki_page_user_prompt(
    page: &super::wiki::WikiOutlinePage,
    files: &[super::wiki::WikiFileContent],
) -> String {
    format!(
        "Wiki page: {}\nCoverage: {}\n\n{}",
        page.title,
        page.description,
        wiki_files_section(files)
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

/// agent 页面 prompt(混合模式):相关文件全文直接喂入(与内置后端同预算同行号前缀),
/// agent 仅在不足时少量补读,不再逐文件工具调用
fn agent_wiki_page_prompt(
    page: &super::wiki::WikiOutlinePage,
    files: &[super::wiki::WikiFileContent],
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
    result: &super::agent::AcpPromptResult,
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
            let input = super::usage::estimate_text_tokens(model, prompt);
            let output = super::usage::estimate_text_tokens(model, &result.text);
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

fn should_reject_wiki_backend_change(
    previous_backend: Option<&str>,
    current_backend: &str,
    automatic: bool,
) -> bool {
    !automatic && previous_backend.unwrap_or("builtin") != current_backend
}

async fn generate_builtin_outline_pages(
    app: &AppHandle,
    db: &Db,
    context: &super::wiki::WikiContext,
    project_name: &str,
    language: &str,
    cancel: &CancellationToken,
    on_retry: impl Fn(WikiRetryNotice),
) -> AppResult<Vec<super::wiki::WikiOutlinePage>> {
    let config = sdk::load_config(app);
    let system = fixed_system_prompt(DEFAULT_WIKI_OUTLINE_PROMPT, language);
    let original_prompt = wiki_outline_user_prompt(context, project_name);
    let mut prompt = original_prompt.clone();
    let valid_files: HashSet<String> = context.paths.iter().cloned().collect();
    let mut last_error = "wiki outline JSON was not generated".to_string();
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        let started = Instant::now();
        let output = match sdk::stream_chat(&config, &system, &prompt, true, cancel, |_| {}).await {
            Ok(output) => output,
            Err(error)
                if !cancel.is_cancelled()
                    && attempt < MAX_ATTEMPTS
                    && error.is_retryable_ai_error() =>
            {
                let notice = retry_notice(&error, attempt, MAX_ATTEMPTS);
                on_retry(notice.clone());
                wait_for_wiki_retry(cancel, &notice).await?;
                continue;
            }
            Err(error) => return Err(error),
        };
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
    on_retry: impl Fn(WikiRetryNotice),
) -> AppResult<()> {
    let config = sdk::load_config(app);
    let system = fixed_system_prompt(DEFAULT_WIKI_PAGE_PROMPT, language);
    let files = super::wiki::read_wiki_files_in(project_path, &page.relevant_files)?;
    let prompt = wiki_page_user_prompt(page, &files);
    let mut last_error = None;
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
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
            Err(error) => {
                if attempt < MAX_ATTEMPTS && error.is_retryable_ai_error() {
                    let notice = retry_notice(&error, attempt, MAX_ATTEMPTS);
                    on_retry(notice.clone());
                    wait_for_wiki_retry(cancel, &notice).await?;
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::coded(ErrorCode::AiRequestFailed, "wiki page generation failed")
    }))
}

/// agent 后端的会话参数。每个页面(及每次重试)独立建会话:长会话上下文累积是
/// max_tokens/max_turn_requests 类中断的主因,独立会话让每个 prompt 都从干净上下文开始;
/// 也正因为会话互不共享状态,页面可以按 concurrency 并行生成。
#[derive(Clone)]
struct AgentSessionParams {
    agent_id: Option<String>,
    custom_command: Option<String>,
    cwd: String,
    model: Option<String>,
    thinking: Option<String>,
    /// 页面并发数(1-8,默认 2)
    concurrency: usize,
}

impl AgentSessionParams {
    fn from_backend(backend: &WikiGenerationBackend, cwd: &str) -> Option<Self> {
        let WikiGenerationBackend::Agent {
            agent_id,
            custom_command,
            model,
            thinking,
            concurrency,
        } = backend
        else {
            return None;
        };
        Some(Self {
            agent_id: agent_id.clone(),
            custom_command: custom_command.clone(),
            cwd: cwd.to_string(),
            model: model.clone(),
            thinking: thinking.clone(),
            concurrency: concurrency.filter(|c| *c > 0).unwrap_or(2).clamp(1, 8),
        })
    }

    /// 用量/元信息里的模型标识:agent 名称 · 所选模型(未选模型则只有名称)
    fn usage_model(&self, agent_name: &str) -> String {
        self.model
            .as_ref()
            .map(|model| format!("{agent_name} · {model}"))
            .unwrap_or_else(|| agent_name.to_string())
    }
}

/// 当前进行中的 agent 会话集合;整本生成期间随大纲/页面推进轮换(并发页面
/// 会同时存在多个会话),取消时终止集合内全部会话(杀进程树)
#[derive(Clone, Default)]
struct AgentSessionSlot(Arc<Mutex<HashSet<String>>>);

impl AgentSessionSlot {
    fn track(&self, run_id: &str) {
        self.0.lock().unwrap().insert(run_id.to_string());
    }

    fn untrack(&self, run_id: &str) {
        self.0.lock().unwrap().remove(run_id);
    }

    fn cancel_all(&self) {
        let ids: Vec<String> = self.0.lock().unwrap().drain().collect();
        for id in ids {
            let _ = super::agent::acp_cancel(id);
        }
    }
}

/// 取消监视:运行取消时终止槽位里的全部 agent 会话
fn watch_agent_cancel(
    token: CancellationToken,
    slot: AgentSessionSlot,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        token.cancelled().await;
        slot.cancel_all();
    })
}

/// 建立一次 agent 会话并登记到槽位
async fn open_agent_session(
    params: &AgentSessionParams,
    slot: &AgentSessionSlot,
) -> AppResult<super::agent::AcpStartResult> {
    let started = super::agent::acp_start(
        params.agent_id.clone(),
        params.custom_command.clone(),
        params.cwd.clone(),
        params.model.clone(),
        params.thinking.clone(),
    )
    .await?;
    slot.track(&started.run_id);
    Ok(started)
}

/// 结束会话:从槽位移除 + 取消(进程由 acp_cancel 的宽限杀与驱动任务收尾兜底)
fn close_agent_session(slot: &AgentSessionSlot, run_id: &str) {
    slot.untrack(run_id);
    let _ = super::agent::acp_cancel(run_id.to_string());
}

/// 单页生成的运行统计(进度面板展示耗时;usage_model 回填 meta)
struct AgentPageStats {
    duration_ms: u64,
    usage_model: String,
}

async fn generate_agent_outline_pages(
    db: &Db,
    run_id: &str,
    usage_model: &str,
    context: &super::wiki::WikiContext,
    project_name: &str,
    language: &str,
    on_activity: Arc<dyn Fn(String) + Send + Sync>,
    on_retry: Arc<dyn Fn(WikiRetryNotice) + Send + Sync>,
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
            &prompt,
            &result,
            started.elapsed().as_millis() as i64,
        );
        // 非正常停止(max_tokens/max_turn_requests 等):部分 JSON 无纠错价值,
        // 用原始 prompt 重新生成;refusal 重试无意义,快速失败
        if result.stop_reason != "end_turn" {
            if result.stop_reason == "refusal" {
                return Err(AppError::coded(
                    ErrorCode::AgentPromptFailed,
                    "agent 拒绝生成大纲(stop_reason=refusal)",
                ));
            }
            last_error = format!("agent 中途停止(stop_reason={})", result.stop_reason);
            prompt = original_prompt.clone();
            continue;
        }
        let (cleaned, retry) = sanitize_agent_retry_notices(&result.text);
        if let Some(notice) = retry {
            on_retry(notice);
        }
        let outline = sdk::strip_thinking(&cleaned);
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
    params: &AgentSessionParams,
    slot: &AgentSessionSlot,
    page: &super::wiki::WikiOutlinePage,
    language: &str,
    changed_files: &[String],
    cancel: &CancellationToken,
    on_progress: Arc<dyn Fn(String) + Send + Sync>,
    on_activity: Arc<dyn Fn(String) + Send + Sync>,
    on_retry: Arc<dyn Fn(WikiRetryNotice) + Send + Sync>,
) -> AppResult<AgentPageStats> {
    // 混合模式:相关文件全文(与内置后端同预算)直接进 prompt,agent 不再逐文件工具调用
    let files = super::wiki::read_wiki_files_in(&params.cwd, &page.relevant_files)?;
    let prompt = agent_wiki_page_prompt(page, &files, changed_files, language);
    let page_started = Instant::now();
    let mut last_error = None;
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        if cancel.is_cancelled() {
            return Err(AppError::coded(ErrorCode::AgentCanceled, ""));
        }
        // 每次尝试独立会话:重试不再背负上一轮的上下文(可能正是 max_tokens 的成因)
        let started = match open_agent_session(params, slot).await {
            Ok(started) => started,
            Err(error) => {
                // agent 未安装/未登录没有重试意义
                let fatal = error.code() == "agent_not_detected";
                last_error = Some(error);
                if fatal {
                    break;
                }
                if attempt < MAX_ATTEMPTS {
                    let notice = retry_notice(last_error.as_ref().unwrap(), attempt, MAX_ATTEMPTS);
                    on_retry(notice.clone());
                    wait_for_wiki_retry(cancel, &notice).await?;
                }
                continue;
            }
        };
        on_progress(String::new());
        let progress = on_progress.clone();
        let activity = on_activity.clone();
        let retry = on_retry.clone();
        let seen_retry = Arc::new(Mutex::new(None::<(usize, usize)>));
        let sender = super::agent::AcpEventSender::new(move |event| match event {
            super::agent::AcpEvent::Chunk { text } => {
                let (cleaned, notice) = sanitize_agent_retry_notices(&text);
                progress(sdk::strip_thinking(&cleaned));
                if let Some(notice) = notice {
                    let marker = (notice.attempt, notice.max_attempts);
                    let mut seen = seen_retry.lock().unwrap();
                    if seen.as_ref() != Some(&marker) {
                        *seen = Some(marker);
                        retry(notice);
                    }
                }
            }
            super::agent::AcpEvent::Activity { text } => activity(text),
        });
        let attempt_started = Instant::now();
        let outcome =
            super::agent::acp_prompt_with(started.run_id.clone(), prompt.clone(), sender).await;
        close_agent_session(slot, &started.run_id);
        let usage_model = params.usage_model(&started.agent_name);
        match outcome {
            Ok(result) => {
                record_acp_usage(
                    db,
                    &usage_model,
                    &prompt,
                    &result,
                    attempt_started.elapsed().as_millis() as i64,
                );
                match result.stop_reason.as_str() {
                    "end_turn" => {
                        let (cleaned, _) = sanitize_agent_retry_notices(&result.text);
                        let page_content = sdk::strip_thinking(&cleaned);
                        if page_content.is_empty() {
                            last_error = Some(AppError::coded(
                                ErrorCode::AgentPromptFailed,
                                "wiki page response is empty after removing thinking content",
                            ));
                        } else {
                            super::wiki::save_wiki_page_internal(
                                app,
                                &params.cwd,
                                &page.file,
                                &page_content,
                            )?;
                            return Ok(AgentPageStats {
                                duration_ms: page_started.elapsed().as_millis() as u64,
                                usage_model,
                            });
                        }
                    }
                    // 模型拒绝:重试同一 prompt 无意义,快速失败
                    "refusal" => {
                        return Err(AppError::coded(
                            ErrorCode::AgentPromptFailed,
                            "agent 拒绝生成该页(stop_reason=refusal)",
                        ));
                    }
                    // max_tokens/max_turn_requests/unknown:下次尝试换新会话重试
                    other => {
                        last_error = Some(AppError::coded(
                            ErrorCode::AgentPromptFailed,
                            format!("agent 中途停止(stop_reason={other})"),
                        ));
                    }
                }
            }
            Err(error) => {
                if cancel.is_cancelled() || error.code() == "agent_canceled" {
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
        if attempt < MAX_ATTEMPTS {
            let notice = retry_notice(last_error.as_ref().unwrap(), attempt, MAX_ATTEMPTS);
            on_retry(notice.clone());
            wait_for_wiki_retry(cancel, &notice).await?;
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

    if let Err(error) = super::wiki::begin_wiki(app.clone(), request.project_path.clone()) {
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
) -> AppResult<WikiUpdateResult> {
    let run = RegisteredRun::new(request.run_id);
    let Some(data) = super::wiki::load_wiki(app.clone(), request.project_path.clone())? else {
        return Ok(WikiUpdateResult::default());
    };
    let backend = super::wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    let Some(from_sha) = data.meta.head_sha.clone() else {
        if request.automatic {
            return Ok(WikiUpdateResult::default());
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
    let changed = match super::wiki::wiki_changed_files(request.project_path.clone(), from_sha) {
        Ok(changed) => changed,
        Err(_) if request.automatic => return Ok(WikiUpdateResult::default()),
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
    Ok(WikiUpdateResult {
        updated_page_ids: affected.into_iter().map(|page| page.id).collect(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        sanitize_agent_retry_notices, should_reject_wiki_backend_change, WikiGenerationEvent,
        WikiUpdateResult,
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
