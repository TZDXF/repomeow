use std::path::{Component, Path};

use crate::commands::git::{commit_blocking, run_git};
use crate::path_util::{clean_str, to_forward_slash_str};

use super::types::{CommitCodeInput, CommitCodeOutput};
use super::util::ToolFailure;

pub(super) fn commit_code_impl(input: CommitCodeInput) -> Result<CommitCodeOutput, ToolFailure> {
    let directory = input.directory.trim();
    if directory.is_empty() || !Path::new(directory).is_dir() {
        return Err(ToolFailure::new(
            "invalid_directory",
            "代码提交目录不存在或不是文件夹",
        ));
    }
    if input.message.trim().is_empty() {
        return Err(ToolFailure::new(
            "git_commit_message_required",
            "Git 提交信息不能为空",
        ));
    }

    let root_output = run_git(directory, &["rev-parse", "--show-toplevel"])
        .map_err(|error| ToolFailure::from_app("无法定位 Git 仓库根目录", error))?;
    let root = String::from_utf8_lossy(&root_output.stdout)
        .trim()
        .to_string();
    if root.is_empty() {
        return Err(ToolFailure::new(
            "not_git_repository",
            "指定目录不是有效的 Git 工作区",
        ));
    }
    let root = clean_str(&root);
    let selected_files = normalize_commit_paths(input.files)?;

    let pathspecs = selected_files.as_ref().map(|paths| {
        paths
            .iter()
            .map(|path| format!(":(literal){path}"))
            .collect()
    });
    let status = commit_blocking(
        &root,
        input.message.trim(),
        selected_files.is_none(),
        pathspecs,
    );

    let status = status.map_err(|error| ToolFailure::from_app("代码提交失败", error))?;
    let hash = git_output(&root, &["rev-parse", "HEAD"], "读取提交哈希失败")?;
    let short_hash = git_output(
        &root,
        &["rev-parse", "--short", "HEAD"],
        "读取短提交哈希失败",
    )?;
    let committed_files = committed_files(&root)?;

    Ok(CommitCodeOutput {
        directory: root,
        commit_hash: hash,
        short_hash,
        branch: status.branch,
        committed_files,
    })
}

pub(super) fn normalize_commit_paths(files: Option<Vec<String>>) -> Result<Option<Vec<String>>, ToolFailure> {
    let Some(files) = files else {
        return Ok(None);
    };
    if files.is_empty() {
        return Err(ToolFailure::new(
            "git_paths_required",
            "files 已提供时至少需要包含一个文件路径",
        ));
    }

    let mut normalized = Vec::with_capacity(files.len());
    for raw in files {
        let trimmed = raw.trim();
        let forward = to_forward_slash_str(trimmed);
        let looks_like_drive_path = forward.as_bytes().get(1) == Some(&b':');
        let invalid_component = Path::new(trimmed)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)));
        let invalid_forward_component = forward
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
        if trimmed.is_empty()
            || trimmed.contains('\0')
            || Path::new(trimmed).is_absolute()
            || forward.starts_with('/')
            || looks_like_drive_path
            || invalid_component
            || invalid_forward_component
        {
            return Err(ToolFailure::new(
                "invalid_file_path",
                format!("提交文件必须是仓库内的相对路径：{raw}"),
            ));
        }
        if !normalized.contains(&forward) {
            normalized.push(forward);
        }
    }
    Ok(Some(normalized))
}

pub(super) fn git_output(root: &str, args: &[&str], message: &str) -> Result<String, ToolFailure> {
    let output = run_git(root, args).map_err(|error| ToolFailure::from_app(message, error))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(super) fn committed_files(root: &str) -> Result<Vec<String>, ToolFailure> {
    let output = run_git(
        root,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "HEAD",
        ],
    )
    .map_err(|error| ToolFailure::from_app("读取本次提交文件失败", error))?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| to_forward_slash_str(&String::from_utf8_lossy(path)))
        .collect())
}
