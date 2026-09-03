use std::fs;
use std::path::{Path};

use serde_json::Value;
use tauri::AppHandle;

use crate::ai::cc_switch;
use crate::commands::files;
use crate::error::{AppError, AppResult, ErrorCode};

// ── cc-switch 导出(勾选 → 写项目文件) ────────────────────────────────

/// 勾选/取消一个 cc-switch 技能:导出到项目 `.claude/skills/<directory>`
/// (已存在则整体替换),取消则删除对应目录。
#[tauri::command]
pub fn set_project_cc_skill(
    app: AppHandle,
    path: String,
    directory: String,
    enable: bool,
) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let cc_dir = cc_switch::cc_switch_dir(&app)?;
    set_project_cc_skill_at(&cc_dir, Path::new(&path), &directory, enable)
}

pub(super) fn set_project_cc_skill_at(
    cc_dir: &Path,
    project: &Path,
    directory: &str,
    enable: bool,
) -> AppResult<()> {
    // directory 只能是单层目录名,防路径穿越
    if directory.is_empty()
        || directory == "."
        || directory == ".."
        || directory.contains(['/', '\\'])
    {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            format!("非法的技能目录名:{directory}"),
        ));
    }
    let target = project.join(".claude").join("skills").join(directory);
    if enable {
        let source = cc_dir.join("skills").join(directory);
        if !source.is_dir() {
            return Err(AppError::coded(
                ErrorCode::InvalidPath,
                format!("cc-switch 中不存在该技能:{directory}"),
            ));
        }
        // 已存在则整体替换,保证与 cc-switch 源一致
        remove_dir_robust(&target)?;
        copy_dir_recursive(&source, &target)?;
    } else {
        remove_dir_robust(&target)?;
    }
    Ok(())
}

/// 递归复制目录;跳过符号链接(不跟随逃逸)。
pub(super) fn copy_dir_recursive(source: &Path, target: &Path) -> AppResult<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 删除目录,容忍不存在;Windows 上先清只读位再重试(skill 目录可能含
/// 只读的 git 对象文件,裸 remove_dir_all 会「拒绝访问」,同 wiki remove_wiki_dir)。
pub(super) fn remove_dir_robust(dir: &Path) -> AppResult<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            clear_readonly_recursive(dir);
            match fs::remove_dir_all(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}

#[cfg(windows)]
pub(super) fn clear_readonly_recursive(root: &Path) {
    fn visit(path: &Path) {
        let Ok(meta) = fs::metadata(path) else {
            return;
        };
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    visit(&entry.path());
                }
            }
        }
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
    visit(root);
}

#[cfg(not(windows))]
pub(super) fn clear_readonly_recursive(_root: &Path) {}

/// 勾选/取消一个 cc-switch MCP 服务器:按 name 合并写入项目 `.mcp.json`
/// 的 mcpServers(项目自有条目不动),取消则移除该键。
/// disable 时若 cc-switch 库中已查不到该 id,用前端传入的 server_name 移除。
#[tauri::command]
pub fn set_project_cc_mcp(
    app: AppHandle,
    path: String,
    server_id: String,
    server_name: String,
    enable: bool,
) -> AppResult<()> {
    files::ensure_dir(&path)?;
    let cc_dir = cc_switch::cc_switch_dir(&app)?;
    let target = Path::new(&path).join(".mcp.json");
    if enable {
        let (name, config) = cc_switch::mcp_server_by_id(&cc_dir, &server_id)?
            .ok_or_else(|| {
                AppError::coded(
                    ErrorCode::InvalidPath,
                    "cc-switch 中已不存在该 MCP 服务器,请刷新列表".to_string(),
                )
            })?;
        upsert_mcp_server(&target, &name, config)
    } else {
        let name = cc_switch::mcp_server_by_id(&cc_dir, &server_id)?
            .map(|(name, _)| name)
            .unwrap_or(server_name);
        remove_mcp_server(&target, &name)
    }
}

/// 读取 `.mcp.json`(不存在则空对象);存在但损坏/非对象时报错,不盲写。
pub(super) fn read_mcp_json(target: &Path) -> AppResult<Value> {
    if !target.is_file() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let raw = fs::read_to_string(target)?;
    let doc: Value = serde_json::from_str(&raw).map_err(|e| {
        AppError::coded(
            ErrorCode::InvalidPath,
            format!("项目 .mcp.json 解析失败({e}),请先手动修复后再操作"),
        )
    })?;
    if !doc.is_object() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            "项目 .mcp.json 顶层不是 JSON 对象,请先手动修复".to_string(),
        ));
    }
    Ok(doc)
}

/// tmp+rename 原子写(末尾换行,2 空格缩进,与常见格式化一致)。
pub(super) fn atomic_write_json(target: &Path, doc: &Value) -> AppResult<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(doc)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    text.push('\n');
    let tmp = target.with_extension("json.repomeow-tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, target)?;
    Ok(())
}

pub(super) fn mcp_servers_map_mut(doc: &mut Value) -> AppResult<&mut serde_json::Map<String, Value>> {
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| AppError::coded(ErrorCode::InvalidPath, ".mcp.json 顶层非对象"))?;
    let entry = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        return Err(AppError::coded(
            ErrorCode::InvalidPath,
            "项目 .mcp.json 的 mcpServers 不是对象,请先手动修复".to_string(),
        ));
    }
    Ok(entry.as_object_mut().unwrap())
}

pub(super) fn upsert_mcp_server(target: &Path, name: &str, config: Value) -> AppResult<()> {
    let mut doc = read_mcp_json(target)?;
    mcp_servers_map_mut(&mut doc)?.insert(name.to_string(), config);
    atomic_write_json(target, &doc)
}

/// 移除键;键不存在时不改写文件(避免无意义的 mtime 变化)。
pub(super) fn remove_mcp_server(target: &Path, name: &str) -> AppResult<()> {
    if !target.is_file() {
        return Ok(());
    }
    let mut doc = read_mcp_json(target)?;
    let removed = doc
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .is_some_and(|servers| servers.remove(name).is_some());
    if removed {
        atomic_write_json(target, &doc)?;
    }
    Ok(())
}

