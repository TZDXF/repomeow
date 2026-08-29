use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::{acp_cancel, acp_prompt_with, acp_start_writable, AcpEventSender};
use crate::background_task::BackgroundTask;
use crate::commands::git;
use crate::error::{AppError, AppResult, ErrorCode};

const FINISHED_EVENT: &str = "agent://conflict-resolution-finished";
static CONFLICT_TASKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn conflict_tasks() -> &'static Mutex<HashSet<String>> {
    CONFLICT_TASKS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Clone, Serialize)]
struct ConflictResolutionFinishedPayload {
    task_id: String,
    project_id: i64,
    path: String,
    success: bool,
    remaining: Vec<String>,
    error: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct RepoSnapshot {
    head: Option<String>,
    state: String,
}

/// 将一次 Git 冲突解决交给本地 ACP agent。命令只负责校验并把工作放入后台；
/// 实际握手、分析、修改和暂存都在任务中进行，页面关闭或切换后仍继续执行。
#[tauri::command]
pub fn resolve_git_conflicts_with_agent(
    app: AppHandle,
    agent_id: String,
    project_id: i64,
    project_name: String,
    path: String,
) -> AppResult<String> {
    let agent_id = agent_id.trim().to_string();
    if agent_id.is_empty() {
        return Err(AppError::coded(
            ErrorCode::AgentNotDetected,
            "未指定用于解决冲突的 agent",
        ));
    }

    let path = crate::path_util::clean_str(&path);
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(
            ErrorCode::GitTaskFailed,
            format!("工作区不存在: {path}"),
        ));
    }
    let initial_snapshot = repo_snapshot(&path)?;
    let conflicts = git::unmerged_files(&path);
    if conflicts.is_empty() {
        return Err(AppError::coded(
            ErrorCode::GitTaskFailed,
            "当前工作区没有待解决的 Git 冲突",
        ));
    }
    if !conflict_tasks().lock().unwrap().insert(path.clone()) {
        return Err(AppError::coded(
            ErrorCode::GitTaskFailed,
            "该工作区已有 Agent 正在解决冲突",
        ));
    }

    let label = format!("{project_name} · Agent");
    let task =
        BackgroundTask::new_for_project(&app, "conflict", label, conflicts.len(), project_id);
    let task_id = task.id().to_string();
    let returned_task_id = task_id.clone();

    tauri::async_runtime::spawn(async move {
        run_resolution_task(
            app,
            task,
            task_id,
            project_id,
            path,
            agent_id,
            conflicts,
            initial_snapshot,
        )
        .await;
    });

    Ok(returned_task_id)
}

async fn run_resolution_task(
    app: AppHandle,
    mut task: BackgroundTask,
    task_id: String,
    project_id: i64,
    path: String,
    agent_id: String,
    conflicts: Vec<String>,
    initial_snapshot: RepoSnapshot,
) {
    let result = run_agent(&agent_id, &path, &conflicts).await;
    let remaining = git::unmerged_files(&path);
    task.set_completed(conflicts.len().saturating_sub(remaining.len()));

    // Agent 直接修改工作区，不经过现有 Git 写命令；在结束时主动刷新并发布统一状态事件。
    if let Ok(status) = git::status(&path) {
        git::publish_write_status(&app, &path, &status, "agent_conflict", false);
    }

    let mut error = result.err().map(error_detail);
    if error.is_none() {
        match repo_snapshot(&path) {
            Ok(current) if current != initial_snapshot => {
                error = Some("Agent 改变了 HEAD 或结束了合并/变基流程，请检查仓库状态".into());
            }
            Err(snapshot_error) => error = Some(error_detail(snapshot_error)),
            _ => {}
        }
    }
    let success = error.is_none() && remaining.is_empty();
    conflict_tasks().lock().unwrap().remove(&path);
    drop(task);
    let _ = app.emit(
        FINISHED_EVENT,
        ConflictResolutionFinishedPayload {
            task_id,
            project_id,
            path,
            success,
            remaining,
            error,
        },
    );
}

fn repo_snapshot(path: &str) -> AppResult<RepoSnapshot> {
    let repo = git::open_repo(path)?
        .ok_or_else(|| AppError::coded(ErrorCode::NotGitRepository, path.to_string()))?;
    let head = repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .map(|oid| oid.to_string());
    Ok(RepoSnapshot {
        head,
        state: format!("{:?}", repo.state()),
    })
}

fn error_detail(error: AppError) -> String {
    match error {
        AppError::Coded { code, message } if message.is_empty() => code.as_str().to_string(),
        AppError::Coded { message, .. } => message,
        other => other.to_string(),
    }
}

async fn run_agent(agent_id: &str, path: &str, conflicts: &[String]) -> AppResult<()> {
    let started = acp_start_writable(agent_id.to_string(), path.to_string()).await?;
    let prompt = conflict_prompt(conflicts);
    let result = acp_prompt_with(started.run_id.clone(), prompt, AcpEventSender::new(|_| {})).await;
    let _ = acp_cancel(started.run_id);
    let result = result?;
    if result.stop_reason != "end_turn" {
        return Err(AppError::coded(
            ErrorCode::AgentPromptFailed,
            format!("agent 提前结束: {}", result.stop_reason),
        ));
    }
    Ok(())
}

fn conflict_prompt(conflicts: &[String]) -> String {
    let files = conflicts
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"请直接解决当前工作区中已有的 Git 合并或变基冲突，不要只给出建议。

冲突文件：
{files}

要求：
1. 先阅读仓库中的 AGENTS.md 等项目约定，并检查冲突两侧及必要的上下文。
2. 只为解决这些冲突做必要修改，正确整合双方意图；不要执行 pull、push、commit、merge/rebase --abort 或 merge/rebase --continue。
3. 移除全部冲突标记，保留可编译、可运行的最终代码；可执行必要的检查或测试。
4. 每解决一个文件后用 `git add -- <path>` 标记为已解决。结束前运行 `git status --short`，确认没有 unmerged 文件。
5. 不要询问用户；自行选择语义上最合理的合并结果。最后简要说明做了什么。"#
    )
}

#[cfg(test)]
mod tests {
    use super::conflict_prompt;

    #[test]
    fn prompt_lists_conflicts_and_forbids_history_operations() {
        let prompt = conflict_prompt(&["src/a.ts".into(), "src/b.rs".into()]);
        assert!(prompt.contains("- src/a.ts"));
        assert!(prompt.contains("- src/b.rs"));
        assert!(prompt.contains("git add -- <path>"));
        assert!(prompt.contains("不要执行 pull、push、commit"));
    }
}
