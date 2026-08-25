mod compose;
mod directory;
mod filename_search;
mod save;
mod text_search;

use crate::error::AppResult;
use crate::models::{ComposeFile, FilePreview, ProjectFileEntry, TextSearchOutcome};

/// 二进制嗅探:读取前缀内出现 NUL 字节即视为二进制(与 git diff 的嗅探口径一致)
const BINARY_SNIFF_BYTES: usize = 8_000;

pub(crate) use compose::compose_files_from_files;
pub(crate) use directory::{ensure_dir, read_readme};

#[cfg_attr(not(test), allow(dead_code))]
pub fn scan_compose_files(path: String) -> AppResult<Vec<ComposeFile>> {
    compose::scan_compose_files(path)
}

#[tauri::command]
pub fn list_project_files(path: String, dir: Option<String>) -> AppResult<Vec<ProjectFileEntry>> {
    directory::list_project_files(path, dir)
}

#[tauri::command]
pub fn search_project_files(
    path: String,
    query: String,
    limit: Option<u32>,
) -> AppResult<Vec<ProjectFileEntry>> {
    filename_search::search_project_files(path, query, limit)
}

#[tauri::command]
pub fn read_file_preview(root: String, rel_path: String) -> AppResult<FilePreview> {
    directory::read_file_preview(root, rel_path)
}

#[tauri::command]
pub fn search_project_text(
    root: String,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    use_regex: bool,
    include: String,
    exclude: String,
) -> AppResult<TextSearchOutcome> {
    text_search::search_project_text(
        root,
        query,
        case_sensitive,
        whole_word,
        use_regex,
        include,
        exclude,
    )
}

#[tauri::command]
pub fn save_text_file(path: String, content: String) -> AppResult<()> {
    save::save_text_file(path, content)
}

#[cfg(test)]
use compose::{maybe_compose_file, parse_compose};
#[cfg(test)]
use directory::PREVIEW_MAX_BYTES;
#[cfg(test)]
use save::SAVE_TEXT_MAX_BYTES;
#[cfg(test)]
use text_search::SEARCH_MAX_MATCHES;

#[cfg(test)]
mod tests;
