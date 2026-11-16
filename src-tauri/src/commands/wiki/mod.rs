//! 项目 Wiki:AI 生成的大纲与页面落盘为 `~/.repomeow/wiki/<basename>-<hash>/` 下的
//! `config.json` + `meta.json` + `pages/NN-slug.md` 普通文件(不进 SQLite),用户可直接
//! 查看/编辑/导出。config.json 保存该项目独立的生成后端配置。
//!
//! wiki 目录本身是一个本地 git 仓库；生成流水线由 `commands::ai` 在后端编排。

mod context;
mod paths;
mod snapshot;
mod storage;
mod types;

use tauri::AppHandle;

use crate::error::AppResult;

pub(crate) use context::{collect_wiki_context, read_wiki_files_in};
pub(crate) use paths::{wiki_dir_at, wiki_dir_in};
pub(crate) use snapshot::wiki_changed_files;
pub(crate) use storage::{
    begin_wiki_at, begin_wiki_page_staging_in, cancel_wiki_page_staging_in, commit_wiki,
    has_wiki_in, load_config_in, load_wiki_at, load_wiki_config_at, load_wiki_config_internal,
    promote_wiki_page_staging_in, read_wiki_page_staging_in, remove_wiki_dir, save_config_in,
    save_wiki_meta_at,
};
pub use types::*;

/// One writer per project across GUI and CLI. Refuse a conflicting operation
/// rather than mixing pages, metadata and git snapshots from separate runs.
pub(crate) fn operation_guard(dir: &std::path::Path) -> AppResult<std::fs::File> {
    let root = dir
        .parent()
        .ok_or_else(|| std::io::Error::other("Invalid wiki directory"))?;
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.with_extension("writer.lock"))?;
    file.try_lock().map_err(|error| {
        std::io::Error::other(format!("Wiki 正在被另一操作写入，请稍后重试: {error}"))
    })?;
    Ok(file)
}

#[tauri::command]
pub fn get_wiki_dir(app: AppHandle, project_path: String) -> AppResult<String> {
    storage::get_wiki_dir(app, project_path)
}

#[tauri::command]
pub fn load_wiki_config(app: AppHandle, project_path: String) -> AppResult<WikiGenerationConfig> {
    storage::load_wiki_config(app, project_path)
}

#[tauri::command]
pub fn save_wiki_config(
    app: AppHandle,
    project_path: String,
    config: WikiGenerationConfig,
) -> AppResult<()> {
    storage::save_wiki_config(app, project_path, config)
}

#[tauri::command]
pub fn has_wiki(app: AppHandle, project_path: String) -> AppResult<bool> {
    storage::has_wiki(app, project_path)
}

#[tauri::command]
pub fn load_wiki(app: AppHandle, project_path: String) -> AppResult<Option<WikiData>> {
    storage::load_wiki(app, project_path)
}

#[tauri::command]
pub fn delete_wiki(app: AppHandle, project_path: String) -> AppResult<()> {
    storage::delete_wiki(app, project_path)
}

#[tauri::command]
pub fn open_wiki_dir(app: AppHandle, project_path: String) -> AppResult<()> {
    storage::open_wiki_dir(app, project_path)
}

#[cfg(test)]
mod tests;
