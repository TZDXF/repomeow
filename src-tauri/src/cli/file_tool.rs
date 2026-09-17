//! CLI `file` 分组:项目文件浏览/搜索/写入。

use std::path::Path;

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

/// 写入项目内文本文件(创建或覆盖;内容上限 512KB,父目录必须已存在)。
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
    if rel.starts_with('/') || rel.split('/').any(|seg| seg == "..") {
        return Err(ToolFailure::new(
            "invalid_file_path",
            "文件路径必须是项目内的相对路径(拒绝绝对路径与 ..)",
        ));
    }
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
    let target = Path::new(&root).join(&rel).to_string_lossy().to_string();
    files::save_text_file(target.clone(), body)
        .map_err(|error| ToolFailure::from_app("写入文件失败", error))?;
    Ok(json!({ "path": rel, "saved": true }))
}
