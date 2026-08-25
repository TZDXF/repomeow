use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::commands::git;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;
use crate::APP_DATA_DIR_NAME;

const WIKI_DIR_NAME: &str = "wiki";

/// FNV-1a 64 位:自实现保证跨版本稳定(std 的 DefaultHasher 不承诺哈希值稳定)
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// wiki 文件夹名:`<basename>-<clean 路径哈希低32位 hex>`。
/// basename 取归一化路径最后一段(非法文件名字符替换为 `_`),哈希防同名碰撞
pub(super) fn folder_name(project_path: &str) -> String {
    let clean = clean_str(project_path);
    let base = clean.rsplit(['\\', '/']).next().unwrap_or_default();
    let base: String = base
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let base = if base.is_empty() { "root" } else { &base };
    format!("{base}-{:08x}", fnv1a64(&clean) as u32)
}

pub(super) fn wiki_dir_in(root: &Path, project_path: &str) -> PathBuf {
    root.join(folder_name(project_path))
}

pub(super) fn wiki_dir(app: &AppHandle, project_path: &str) -> AppResult<PathBuf> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    Ok(wiki_dir_in(
        &home.join(APP_DATA_DIR_NAME).join(WIKI_DIR_NAME),
        project_path,
    ))
}

/// 读取仓库当前 HEAD 的完整 sha;非 git 仓库 / 空仓库 / 读取失败均为 None
pub(super) fn head_sha(project_path: &str) -> Option<String> {
    let repo = git::open_repo(project_path).ok()??;
    let sha = repo.head().ok()?.target().map(|oid| oid.to_string());
    sha
}
