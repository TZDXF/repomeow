//! CLI `file` 分组:项目文件浏览/搜索/写入。

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::commands::files;

use super::util::ToolFailure;
use crate::path_util::{clean_str, to_forward_slash_str};

fn require_project_root(project_directory: &str) -> Result<String, ToolFailure> {
    let root = clean_str(project_directory);
    if root.is_empty() || !Path::new(&root).is_dir() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不存在或不是文件夹",
        ));
    }
    Ok(root)
}

/// 列出项目目录下一层条目(文件/文件夹)。
pub(super) fn list_files_impl(
    project_directory: &str,
    dir: Option<&str>,
) -> Result<Value, ToolFailure> {
    let root = require_project_root(project_directory)?;
    let entries = files::list_project_files(root, dir.map(str::to_string))
        .map_err(|error| ToolFailure::from_app("列出项目文件失败", error))?;
    Ok(json!({ "entries": entries }))
}

/// 按文件名模糊搜索项目文件。
pub(super) fn search_files_impl(
    project_directory: &str,
    query: &str,
    limit: Option<u32>,
) -> Result<Value, ToolFailure> {
    let root = require_project_root(project_directory)?;
    if query.trim().is_empty() {
        return Err(ToolFailure::new("invalid_query", "搜索关键词不能为空"));
    }
    let entries = files::search_project_files(root, query.to_string(), limit)
        .map_err(|error| ToolFailure::from_app("搜索项目文件失败", error))?;
    Ok(json!({ "entries": entries }))
}

/// 全文搜索项目文本内容(glob 过滤语法与设置页一致)。
#[allow(clippy::too_many_arguments)]
pub(super) fn search_text_impl(
    project_directory: &str,
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<Value, ToolFailure> {
    let root = require_project_root(project_directory)?;
    if query.is_empty() {
        return Err(ToolFailure::new("invalid_query", "搜索关键词不能为空"));
    }
    let outcome = files::search_project_text(
        root,
        query.to_string(),
        case_sensitive,
        whole_word,
        regex,
        include.unwrap_or_default().to_string(),
        exclude.unwrap_or_default().to_string(),
    )
    .map_err(|error| ToolFailure::from_app("搜索项目文本失败", error))?;
    Ok(json!(outcome))
}

fn invalid_save_path() -> ToolFailure {
    ToolFailure::new(
        "invalid_file_path",
        "文件路径必须是项目内的相对路径,且不能通过符号链接或 junction 逃逸",
    )
}

/// 将待写路径限制在项目根目录内。父目录必须存在;已有目标不允许是符号链接。
fn resolve_save_target(root: &str, rel: &str) -> Result<PathBuf, ToolFailure> {
    let rel_path = Path::new(rel);
    let bytes = rel.as_bytes();
    let has_windows_drive_prefix =
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if rel_path.is_absolute()
        || has_windows_drive_prefix
        || !rel_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(invalid_save_path());
    }

    let root = std::fs::canonicalize(root).map_err(|_| invalid_save_path())?;
    let target = root.join(rel_path);
    let parent = target.parent().ok_or_else(invalid_save_path)?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|_| invalid_save_path())?;
    if !canonical_parent.is_dir() || !canonical_parent.starts_with(&root) {
        return Err(invalid_save_path());
    }

    match std::fs::symlink_metadata(&target) {
        Ok(metadata) => {
            // 已存在的符号链接即使当前指向项目内也拒绝,避免写入目标被替换。
            if metadata.file_type().is_symlink() {
                return Err(invalid_save_path());
            }
            let canonical_target =
                std::fs::canonicalize(&target).map_err(|_| invalid_save_path())?;
            if !canonical_target.is_file() || !canonical_target.starts_with(&root) {
                return Err(invalid_save_path());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(invalid_save_path()),
    }
    Ok(target)
}

/// 写入项目内文本文件(创建或覆盖;内容上限 512KB,父目录必须已存在)。
/// 拒绝绝对路径、..、符号链接/junction 逃逸及已有符号链接目标。
/// 内容来源:--content 内联文本,或 --content-file 从文件读入(两者取一)。
pub(super) fn save_file_impl(
    project_directory: &str,
    path: &str,
    content: Option<&str>,
    content_file: Option<&str>,
) -> Result<Value, ToolFailure> {
    let root = require_project_root(project_directory)?;
    let rel = to_forward_slash_str(path.trim());
    if rel.is_empty() {
        return Err(ToolFailure::new("invalid_file_path", "文件路径不能为空"));
    }
    let target = resolve_save_target(&root, &rel)?;
    let body = match (content, content_file) {
        (Some(_), Some(_)) => {
            return Err(ToolFailure::new(
                "invalid_arguments",
                "--content 与 --content-file 只能二选一",
            ))
        }
        (Some(text), None) => text.to_string(),
        (None, Some(file)) => std::fs::read_to_string(file).map_err(|error| {
            ToolFailure::new("content_file_read_failed", "读取内容文件失败")
                .with_detail(format!("{file}: {error}"))
        })?,
        (None, None) => {
            return Err(ToolFailure::new(
                "invalid_arguments",
                "必须通过 --content 或 --content-file 提供写入内容",
            ))
        }
    };
    files::save_text_file(target.to_string_lossy().into_owned(), body)
        .map_err(|error| ToolFailure::from_app("写入文件失败", error))?;
    Ok(json!({ "path": rel, "saved": true }))
}
