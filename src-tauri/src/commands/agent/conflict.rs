use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::agent::harness::restricted_env::RestrictedEnv;
use crate::agent::harness::runtime::harness_tool_from_core;
use crate::agent::harness::tools::index::{
    create_edit_tool, create_find_tool, create_grep_tool, create_ls_tool, create_read_tool,
    create_write_tool,
};
use crate::background_task::BackgroundTask;
use crate::commands::ai::harness_support::{
    builtin_stream_fn, collect_usage_events, create_harness, effective_thinking_level,
    load_builtin_agent_model, prompt_with_timeout, record_collected_usage, BuiltinAgentModel,
};
use crate::commands::git;
use crate::error::{AppError, AppResult, ErrorCode};
use std::sync::Arc;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

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

/// 内置 Agent 逐文件受限修复，禁止 Shell；应用校验结果后暂存。
#[tauri::command]
pub fn resolve_git_conflicts_with_agent(
    app: AppHandle,
    model: Option<String>,
    thinking: Option<String>,
    project_id: i64,
    project_name: String,
    path: String,
) -> AppResult<String> {
    let configured = load_builtin_agent_model(&app, model.as_deref())?;
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
            configured,
            thinking,
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
    configured: BuiltinAgentModel,
    thinking: Option<String>,
    conflicts: Vec<String>,
    initial_snapshot: RepoSnapshot,
) {
    let result = run_agent(
        &app,
        configured,
        thinking.as_deref(),
        &path,
        &conflicts,
        &initial_snapshot,
    )
    .await;
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

async fn run_agent(
    app: &AppHandle,
    configured: BuiltinAgentModel,
    thinking: Option<&str>,
    path: &str,
    conflicts: &[String],
    initial: &RepoSnapshot,
) -> AppResult<()> {
    let root = std::path::Path::new(path);
    let level = effective_thinking_level(&configured.model, thinking);
    for file in conflicts {
        let target = root.join(file);
        // 仅处理带文本冲突标记的普通文件；删除/二进制/链接冲突留给用户。
        if !target.canonicalize()?.starts_with(root.canonicalize()?) {
            return Err(AppError::coded(
                ErrorCode::InvalidPath,
                "冲突文件位于仓库之外",
            ));
        }
        {
            let repo = git::open_repo(path)?
                .ok_or_else(|| AppError::coded(ErrorCode::NotGitRepository, path))?;
            let index = repo.index().map_err(git_error)?;
            let relative = std::path::Path::new(file);
            let ours = index.get_path(relative, 2);
            let theirs = index.get_path(relative, 3);
            if ours.is_none()
                || theirs.is_none()
                || [ours, theirs]
                    .iter()
                    .flatten()
                    .any(|entry| !matches!(entry.mode, 0o100644 | 0o100755))
            {
                return Err(AppError::coded(
                    ErrorCode::GitTaskFailed,
                    format!("需手动处理删除或特殊文件冲突: {file}"),
                ));
            }
        }
        let metadata = std::fs::symlink_metadata(&target)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AppError::coded(
                ErrorCode::GitTaskFailed,
                format!("需手动处理非文本冲突: {file}"),
            ));
        }
        let before = std::fs::read_to_string(&target)?;
        if !has_conflict_markers(&before) {
            return Err(AppError::coded(
                ErrorCode::GitTaskFailed,
                format!("需手动处理无文本标记的冲突: {file}"),
            ));
        }
        let env = RestrictedEnv::for_agent(root, &target)
            .map_err(|e| AppError::coded(ErrorCode::InvalidPath, e.to_string()))?;
        let tools = vec![
            create_read_tool(env.clone(), None),
            create_grep_tool(env.clone()),
            create_find_tool(env.clone()),
            create_ls_tool(env.clone()),
            create_write_tool(env.clone()),
            create_edit_tool(env),
        ]
        .into_iter()
        .map(harness_tool_from_core)
        .collect();
        let cancel = CancellationToken::new();
        let request_cancel = cancel.child_token();
        let harness = Arc::new(create_harness(configured.model.clone(), builtin_stream_fn(configured.api_key.clone(), request_cancel.clone()), tools,
            "You are RepoMeow's internal conflict resolution agent. Only edit the specified conflict file. Shell and Git operations are unavailable.".into(), level.clone()).await?);
        let usages = collect_usage_events(&harness).await;
        let result = prompt_with_timeout(
            harness,
            conflict_prompt(&[file.clone()]),
            &cancel,
            &request_cancel,
        )
        .await;
        record_collected_usage(
            &app.state::<crate::db::Db>(),
            "conflict",
            &configured.model.id,
            &usages.lock().unwrap(),
        );
        result?;
        let after = std::fs::read_to_string(&target)?;
        if after == before || has_conflict_markers(&after) || repo_snapshot(path)? != *initial {
            return Err(AppError::coded(
                ErrorCode::GitTaskFailed,
                format!("冲突未解决或仓库状态已改变: {file}"),
            ));
        }
        // 不运行 git add(避免外部 clean filter)，直接将文本写入对象库与索引。
        let repo = git::open_repo(path)?
            .ok_or_else(|| AppError::coded(ErrorCode::NotGitRepository, path))?;
        let mut index = repo.index().map_err(git_error)?;
        let relative = std::path::Path::new(file);
        let mut entry = index
            .get_path(relative, 2)
            .or_else(|| index.get_path(relative, 3))
            .ok_or_else(|| AppError::coded(ErrorCode::GitTaskFailed, "冲突索引已改变"))?;
        entry.flags &= !0x3000;
        index
            .add_frombuffer(&entry, after.as_bytes())
            .map_err(git_error)?;
        index.write().map_err(git_error)?;
    }
    Ok(())
}

fn git_error(error: git2::Error) -> AppError {
    AppError::coded(ErrorCode::GitTaskFailed, error.to_string())
}

fn has_conflict_markers(content: &str) -> bool {
    content.lines().any(|line| {
        ["<<<<<<<", "=======", ">>>>>>>", "|||||||"]
            .iter()
            .any(|marker| line.starts_with(marker))
    })
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
3. 移除全部冲突标记，保留可编译、可运行的最终代码。无 Shell 权限，不执行测试。
4. 仅使用 read/grep/find/ls 读取上下文，用 edit/write 修改指定文件；应用将校验并暂存，不要执行 Git 命令。
5. 不要询问用户；自行选择语义上最合理的合并结果。最后简要说明做了什么。"#
    )
}

#[cfg(test)]
mod tests {
    use super::{conflict_prompt, has_conflict_markers};

    #[tokio::test]
    async fn conflict_environment_restricts_writes() {
        use crate::agent::harness::restricted_env::RestrictedEnv;
        let root = std::env::temp_dir().join(format!(
            "repomeow-conflict-{}",
            crate::time_util::now_ts_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("conflict.txt");
        std::fs::write(&target, "conflict").unwrap();
        std::fs::write(root.join("other.txt"), "unchanged").unwrap();
        let env = RestrictedEnv::for_agent(&root, &target).unwrap();
        assert!(env
            .write_file("conflict.txt".into(), "resolved".into(), None)
            .await
            .is_ok());
        assert!(env
            .write_file("other.txt".into(), "bad".into(), None)
            .await
            .is_err());
        assert!(env
            .write_file(".git/config".into(), "bad".into(), None)
            .await
            .is_err());
        assert!(env
            .exec("git push".into(), Default::default())
            .await
            .is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("other.txt")).unwrap(),
            "unchanged"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_remaining_markers() {
        for marker in ["<<<<<<< ours", "=======", ">>>>>>> theirs", "||||||| base"] {
            assert!(has_conflict_markers(&format!("code\n{marker}\n")));
        }
        assert!(!has_conflict_markers("const value = 1;\n"));
    }

    #[test]
    fn prompt_lists_conflicts_and_forbids_history_operations() {
        let prompt = conflict_prompt(&["src/a.ts".into(), "src/b.rs".into()]);
        assert!(prompt.contains("- src/a.ts"));
        assert!(prompt.contains("- src/b.rs"));
        assert!(prompt.contains("不要执行 Git 命令"));
        assert!(prompt.contains("不要执行 pull、push、commit"));
    }
}
