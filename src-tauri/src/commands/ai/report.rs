use std::collections::HashSet;
use std::time::Instant;

use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::ai::prompts::{
    effective_system_prompt, DEFAULT_REPORT_PROMPT, DEFAULT_WEEKLY_REPORT_PROMPT,
};
use crate::ai::sdk;
use crate::commands::{git, report};
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::GitCommitInfo;

use super::run::{record_usage, RegisteredRun};

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
    pub(crate) run_id: String,
    pub(crate) project_ids: Vec<i64>,
    pub(crate) date_from: String,
    pub(crate) date_to: String,
    pub(crate) range_label: String,
    pub(crate) author_mode: String,
    pub(crate) language: String,
    pub(crate) period_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedReport {
    pub(crate) history_id: i64,
    pub(crate) result: String,
    pub(crate) commit_data: Vec<ReportProjectCommits>,
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
                    git::run_git_current_user(&project.path)
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
                let commits = git::run_git_log(
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
    let commit_data: Vec<report::SaveReportCommit> = data
        .iter()
        .map(|project| report::SaveReportCommit {
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
    let history_id = report::save_report_history_impl(
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
