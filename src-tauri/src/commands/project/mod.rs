//! 项目命令域:按职责拆分为 Tauri 命令、目录移动与仓库读写层。
//! - `commands`:`#[tauri::command]` 包装层,负责锁 Db 并调底层
//! - `move_dir`:跨盘/同盘目录移动计划与落盘
//! - `repository`:纯 SQLite 读写,无 Tauri/锁依赖,便于单元测试
mod commands;
mod move_dir;
mod repository;

pub use commands::*;
pub use repository::normalize_stored_paths;

#[cfg(test)]
pub use move_dir::move_dir;
#[cfg(test)]
pub use repository::{
    add, archive, get, list, list_archived, load_tags, remove, set_auto_pull, set_favorite,
    set_wiki_auto_update, unarchive, update, update_path,
};

#[cfg(test)]
mod tests;
