//! 项目 AI 资产的可视化管理命令:skills 的新建/删除,MCP 服务器的
//! 表单新增/修改/移除(写回探测表内的 MCP 配置文件)。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::cc_export::{remove_dir_robust, remove_mcp_server, upsert_mcp_server};
use super::{MCP_PROBES, SKILL_DIR_PROBES};
use crate::commands::files;
use crate::error::{AppError, AppResult, ErrorCode};

/// 校验单层名称(技能目录名 / MCP 服务器键名):去首尾空白后非空、
/// 不含路径分隔符、非 `.` / `..`(防路径穿越)。
fn validate_single_segment(name: &str, what: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            format!("非法的{what}:{name}"),
        ));
    }
    Ok(name.to_string())
}

/// 按探测表解析 MCP 配置文件相对路径 → (绝对目标路径, 服务器对象键名)。
/// 只允许探测表内的文件,拒绝任意路径写。
fn resolve_mcp_target(project: &str, config_path: &str) -> AppResult<(PathBuf, &'static str)> {
    let key = MCP_PROBES
        .iter()
        .find(|(rel, _)| *rel == config_path)
        .map(|(_, key)| *key)
        .ok_or_else(|| {
            AppError::coded(
                ErrorCode::InvalidPath,
                format!("不支持的 MCP 配置文件:{config_path}"),
            )
        })?;
    Ok((Path::new(project).join(config_path), key))
}

/// 新建项目技能:在 `.claude/skills/<name>/` 写入带 frontmatter 的 SKILL.md 模板,
/// 返回新技能目录的仓库相对路径。已存在同名技能时报错。
#[tauri::command]
pub fn create_project_skill(path: String, name: String, description: String) -> AppResult<String> {
    files::ensure_dir(&path)?;
    let name = validate_single_segment(&name, "技能目录名")?;
    let dir = Path::new(&path).join(".claude").join("skills").join(&name);
    if dir.exists() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            format!("技能已存在:{name}"),
        ));
    }
    fs::create_dir_all(&dir)?;
    let mut content = format!("---\nname: {name}\n");
    let description = description.trim();
    if !description.is_empty() {
        // 单行 + 引号包裹,避免冒号/引号破坏 YAML 标量
        let escaped = description
            .replace(['\r', '\n'], " ")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        content.push_str(&format!("description: \"{escaped}\"\n"));
    }
    content.push_str(&format!("---\n\n# {name}\n\n<!-- TODO: 描述技能的用途与使用步骤 -->\n"));
    fs::write(dir.join("SKILL.md"), content)?;
    Ok(format!(".claude/skills/{name}"))
}

/// 删除项目技能整目录;dir 必须是探测 skills 目录下的单层子目录。
#[tauri::command]
pub fn delete_project_skill(path: String, dir: String) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let target = validate_skill_dir(Path::new(&path), &dir)?;
    remove_dir_robust(&target)
}

/// 校验技能目录相对路径并解析为绝对路径(`<探测 skills 目录>/<单层名>`)。
fn validate_skill_dir(project: &Path, dir: &str) -> AppResult<PathBuf> {
    let (prefix, tail) = dir
        .rsplit_once('/')
        .ok_or_else(|| AppError::coded(ErrorCode::InvalidPath, format!("非法的技能目录:{dir}")))?;
    if !SKILL_DIR_PROBES.contains(&prefix) {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            format!("不支持的 skills 目录:{prefix}"),
        ));
    }
    let name = validate_single_segment(tail, "技能目录名")?;
    Ok(project.join(prefix).join(name))
}

/// 表单保存一个 MCP 服务器:按 name 整体写入(同名覆盖,其余条目不动);
/// config 是前端组装好的完整服务器定义,原样落盘。
#[tauri::command]
pub fn set_project_mcp_server(
    path: String,
    config_path: String,
    name: String,
    config: Value,
) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let name = validate_single_segment(&name, "MCP 服务器名")?;
    if !config.is_object() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            "MCP 服务器定义必须是 JSON 对象".to_string(),
        ));
    }
    let (target, key) = resolve_mcp_target(&path, &config_path)?;
    upsert_mcp_server(&target, key, &name, config)
}

/// 移除一个 MCP 服务器条目;键不存在或文件缺失时不动文件。
#[tauri::command]
pub fn remove_project_mcp_server(path: String, config_path: String, name: String) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let name = validate_single_segment(&name, "MCP 服务器名")?;
    let (target, key) = resolve_mcp_target(&path, &config_path)?;
    remove_mcp_server(&target, key, &name)
}
