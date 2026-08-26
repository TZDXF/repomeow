use std::path::Path;

use crate::commands::walk;
use crate::error::AppResult;
use crate::models::ProjectFileEntry;

use super::ensure_dir;

/// 文件名搜索(文件树头部搜索框):在未被 .gitignore / .ignore 排除的文件中
/// 按相对路径做大小写不敏感子串匹配(与原前端「被排除文件不参与」口径一致),
/// 遍历命中 limit 条即提前退出;结果按路径排序。空查询返回空
pub(super) fn search_project_files(
    path: String,
    query: String,
    limit: Option<u32>,
) -> AppResult<Vec<ProjectFileEntry>> {
    ensure_dir(&path)?;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(50).max(1) as usize;
    Ok(walk::search_file_paths(Path::new(&path), &needle, limit)
        .into_iter()
        .map(|p| ProjectFileEntry {
            path: walk::to_slash(&p),
            ignored: false,
            is_dir: false,
        })
        .collect())
}
