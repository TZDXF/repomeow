use std::path::Path;

use serde_json::{json, Value};

use crate::commands::files::read_file_preview;
use crate::commands::report::list_report_history_impl;
use crate::commands::script;

use super::types::{
    ListReportsInput, ProjectDirectoryInput, ProjectFileOutput, ReadProjectFileInput,
};
use super::util::{
    data_root_or_default, open_db, require_project_id, require_project_id_including_archived,
    ToolFailure,
};
use crate::commands::project as project_repo;
use crate::path_util::{clean_str, to_forward_slash_str};

/// read_project_file 默认/最大返回行数(对齐 chat 工具)。
const READ_FILE_DEFAULT_LINES: u64 = 400;
const READ_FILE_MAX_LINES: u64 = 5000;

// ── 项目洞察 ──────────────────────────────────────────────────────────

pub(super) fn read_project_file_impl(
    input: ReadProjectFileInput,
) -> Result<ProjectFileOutput, ToolFailure> {
    let root = clean_str(&input.project_directory);
    if root.is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }
    let rel_path = to_forward_slash_str(input.path.trim());
    if rel_path.is_empty() {
        return Err(ToolFailure::new("invalid_file_path", "文件路径不能为空"));
    }
    let offset_line = input.offset_line.unwrap_or(1).max(1);
    let max_lines = input
        .max_lines
        .unwrap_or(READ_FILE_DEFAULT_LINES)
        .clamp(1, READ_FILE_MAX_LINES) as usize;
    // read_file_preview 内部已做 canonicalize + 根目录前缀校验,拒绝越界与符号链接逃逸。
    let preview = read_file_preview(root, rel_path.clone())
        .map_err(|error| ToolFailure::from_app("读取文件失败", error))?;
    let Some(text) = preview.text else {
        return Err(ToolFailure::new(
            "binary_file",
            format!("「{rel_path}」是二进制或非 UTF-8 文件,无法按行读取"),
        ));
    };
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Ok(ProjectFileOutput {
            path: rel_path,
            total_lines: 0,
            start_line: 0,
            end_line: 0,
            content: String::new(),
            has_more: false,
            preview_truncated: preview.truncated,
        });
    }
    let start = ((offset_line as usize).saturating_sub(1)).min(total);
    if start >= total {
        return Err(ToolFailure::new(
            "offset_out_of_range",
            format!("文件共 {total} 行,offsetLine={offset_line} 超出范围"),
        ));
    }
    let end = (start + max_lines).min(total);
    let content = lines[start..end]
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{}: {}", start + index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ProjectFileOutput {
        path: rel_path,
        total_lines: total,
        start_line: start as u64 + 1,
        end_line: end as u64,
        content,
        has_more: end < total,
        preview_truncated: preview.truncated,
    })
}

pub(super) fn list_reports_impl(
    input: ListReportsInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = match &input.project_directory {
        Some(directory) => Some(require_project_id(&conn, directory)?),
        None => None,
    };
    let limit = Some(input.limit.unwrap_or(10).clamp(1, 50) as usize);
    let items = list_report_history_impl(&conn, limit, Some(0), project_id)
        .map_err(|error| ToolFailure::from_app("查询报告历史失败", error))?;
    Ok(json!({ "reports": items }))
}

pub(super) fn list_custom_commands_impl(
    input: ProjectDirectoryInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, &input.project_directory)?;
    let commands = script::list_commands(&conn, project_id)
        .map_err(|error| ToolFailure::from_app("查询自定义命令失败", error))?;
    Ok(json!({
        "projectId": project_id,
        "commands": commands,
    }))
}

// ── 项目登记管理 ──────────────────────────────────────────────────────

pub(super) fn list_projects_impl(
    archived: bool,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let projects = if archived {
        project_repo::list_archived(&conn)
    } else {
        project_repo::list(&conn, None, None)
    }
    .map_err(|error| ToolFailure::from_app("查询项目列表失败", error))?;
    Ok(json!({ "projects": projects }))
}

pub(super) fn get_project_impl(
    input: ProjectDirectoryInput,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let path = clean_str(&input.project_directory);
    let id = require_project_id_including_archived(&conn, &path)?;
    let project = project_repo::get(&conn, id)
        .map_err(|error| ToolFailure::from_app("查询项目失败", error))?;
    Ok(json!(project))
}

pub(super) fn add_project_impl(
    project_directory: &str,
    name: Option<String>,
    description: Option<String>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let path = clean_str(project_directory);
    if path.is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }
    let name = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            // 与前端一致:缺省取目录 basename 作为项目名
            path.rsplit(['/', '\\'])
                .next()
                .unwrap_or(path.as_str())
                .to_string()
        });
    let description = description.unwrap_or_default();
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project = project_repo::add(&conn, &path, &name, &description)
        .map_err(|error| ToolFailure::from_app("登记项目失败", error))?;
    Ok(json!(project))
}

pub(super) fn update_project_impl(
    project_directory: &str,
    name: Option<String>,
    description: Option<String>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let id = require_project_id(&conn, project_directory)?;
    let existing = project_repo::get(&conn, id)
        .map_err(|error| ToolFailure::from_app("查询项目失败", error))?;
    // 缺省字段沿用原值,支持只改单项
    let name = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(existing.name);
    let description = description.unwrap_or(existing.description);
    let project = project_repo::update(&conn, id, &name, &description)
        .map_err(|error| ToolFailure::from_app("更新项目失败", error))?;
    Ok(json!(project))
}

pub(super) fn set_project_archived_impl(
    project_directory: &str,
    archived: bool,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let id = require_project_id_including_archived(&conn, project_directory)?;
    let result = if archived {
        project_repo::archive(&conn, id)
    } else {
        project_repo::unarchive(&conn, id)
    };
    result.map_err(|error| {
        ToolFailure::from_app(
            if archived {
                "归档项目失败"
            } else {
                "恢复项目失败"
            },
            error,
        )
    })?;
    Ok(json!({ "projectId": id, "archived": archived }))
}

pub(super) fn delete_project_impl(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let id = require_project_id_including_archived(&conn, project_directory)?;
    project_repo::remove(&conn, id)
        .map_err(|error| ToolFailure::from_app("删除项目失败", error))?;
    Ok(json!({ "projectId": id, "deleted": true }))
}

/// 项目开关:favorite(收藏)/ autoPull(跟踪更新)/ wikiAutoUpdate(Wiki 自动更新)。
pub(super) fn set_project_flag_impl(
    project_directory: &str,
    flag: &str,
    enabled: bool,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let id = require_project_id(&conn, project_directory)?;
    let result = match flag {
        "favorite" => project_repo::set_favorite(&conn, id, enabled),
        "autoPull" => project_repo::set_auto_pull(&conn, id, enabled),
        "wikiAutoUpdate" => project_repo::set_wiki_auto_update(&conn, id, enabled),
        _ => {
            return Err(ToolFailure::new(
                "invalid_flag",
                format!("未知项目开关: {flag}"),
            ))
        }
    };
    result.map_err(|error| ToolFailure::from_app("更新项目开关失败", error))?;
    Ok(json!({ "projectId": id, "flag": flag, "enabled": enabled }))
}
