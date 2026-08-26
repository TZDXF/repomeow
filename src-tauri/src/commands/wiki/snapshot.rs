use std::path::Path;

use crate::commands::git;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::to_forward_slash;

use super::types::{WikiChangedFiles, WikiCommitKind, WikiMeta};

const WIKI_GIT_NAME: &str = "RepoMeow";
const WIKI_GIT_EMAIL: &str = "repomeow@localhost";

pub(super) fn commit_message(
    kind: WikiCommitKind,
    meta: &WikiMeta,
    title: Option<&str>,
    head: Option<&str>,
) -> String {
    let short = head.filter(|s| s.len() >= 7).map(|s| &s[..7]);
    match kind {
        WikiCommitKind::Generate => match short {
            Some(s) => format!("生成 wiki(共 {} 页,代码 {s})", meta.outline.len()),
            None => format!("生成 wiki(共 {} 页)", meta.outline.len()),
        },
        WikiCommitKind::Update => match short {
            Some(s) => format!("增量更新 wiki(代码 {s})"),
            None => "增量更新 wiki".into(),
        },
        WikiCommitKind::Page => match (title, short) {
            (Some(t), Some(s)) => format!("重新生成页面:{t}(代码 {s})"),
            (Some(t), None) => format!("重新生成页面:{t}"),
            (None, Some(s)) => format!("重新生成页面(代码 {s})"),
            (None, None) => "重新生成页面".into(),
        },
    }
}

/// 在 wiki 目录做一次快照提交，无变更时幂等跳过。
pub(super) fn commit_wiki_in(dir: &Path, message: &str) -> AppResult<()> {
    let dir_str = dir.to_string_lossy().into_owned();
    if !dir.join(".git").exists() {
        git::run_git(&dir_str, &["init"])?;
        for (key, value) in [
            ("user.name", WIKI_GIT_NAME),
            ("user.email", WIKI_GIT_EMAIL),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git::run_git(&dir_str, &["config", key, value])?;
        }
    }
    let status = git::git_command(&dir_str)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    if !status.status.success() {
        return Err(AppError::coded(
            ErrorCode::GitCommandFailed,
            format!(
                "git status: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            ),
        ));
    }
    if status.stdout.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(());
    }
    git::run_git(&dir_str, &["add", "-A"])?;
    git::run_git(
        &dir_str,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--no-verify",
            "-m",
            message,
        ],
    )?;
    Ok(())
}

/// 增量更新用:列出 from_sha..HEAD 之间变更的文件(/ 分隔相对路径)。
pub(crate) fn wiki_changed_files(
    project_path: String,
    from_sha: String,
) -> AppResult<WikiChangedFiles> {
    let Some(repo) = git::open_repo(&project_path)? else {
        return Ok(WikiChangedFiles {
            files: Vec::new(),
            head_sha: None,
        });
    };
    let oid = git2::Oid::from_str(&from_sha)
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let from = repo
        .find_commit(oid)
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let Ok(head) = repo.head().and_then(|h| h.peel_to_commit()) else {
        return Ok(WikiChangedFiles {
            files: Vec::new(),
            head_sha: None,
        });
    };
    let from_tree = from
        .tree()
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let head_tree = head
        .tree()
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let diff = repo
        .diff_tree_to_tree(Some(&from_tree), Some(&head_tree), None)
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let mut files: Vec<String> = diff
        .deltas()
        .filter_map(|d| {
            let path = if d.status() == git2::Delta::Deleted {
                d.old_file().path()
            } else {
                d.new_file().path()
            };
            path.map(to_forward_slash)
        })
        .collect();
    files.sort();
    files.dedup();
    Ok(WikiChangedFiles {
        files,
        head_sha: Some(head.id().to_string()),
    })
}

#[cfg(test)]
pub(super) const TEST_WIKI_GIT_NAME: &str = WIKI_GIT_NAME;
