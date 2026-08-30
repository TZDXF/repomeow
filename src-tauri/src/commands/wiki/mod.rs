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
pub(crate) use paths::wiki_dir_in;
pub(crate) use snapshot::wiki_changed_files;
pub(crate) use storage::{
    begin_wiki, commit_wiki, load_wiki_config_internal, save_wiki_meta, save_wiki_page_internal,
};
pub use types::*;

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
