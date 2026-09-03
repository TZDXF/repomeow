use std::path::Path;

use crate::commands::usage::count_o200k_tokens;
use crate::commands::walk;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{FilePreview, ProjectFileEntry};

use super::BINARY_SNIFF_BYTES;

pub(crate) fn ensure_dir(path: &str) -> AppResult<()> {
    if !Path::new(path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, path));
    }
    Ok(())
}

/// README 候选文件名,按优先级排列(大小写常见变体)
const README_CANDIDATES: &[&str] = &[
    "README.md",
    "readme.md",
    "README.MD",
    "Readme.md",
    "README.markdown",
    "README.txt",
    "README",
];

/// README 读取上限 512KB
const README_MAX_BYTES: u64 = 512 * 1024;

/// 在目录中按候选名查找文件,返回第一个存在的文件名。
/// 用 read_dir 做大小写精确匹配,避免 Windows/macOS 大小写不敏感文件系统
/// 把 readme.md 误判成 README.md,保证候选优先级在所有平台行为一致。
fn find_file(dir: &Path, candidates: &[&str]) -> Option<String> {
    let existing: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    candidates
        .iter()
        .find(|name| existing.iter().any(|f| f == *name))
        .map(|name| name.to_string())
}

/// 读取项目根目录 README 全文;不存在时返回 None。
/// 仅供后端内部使用(wiki 上下文收集);前端查看 README 走文件预览页
pub(crate) fn read_readme(path: &str) -> AppResult<Option<String>> {
    ensure_dir(path)?;
    let dir = Path::new(path);
    let Some(file_name) = find_file(dir, README_CANDIDATES) else {
        return Ok(None);
    };
    let file = dir.join(&file_name);
    // 超过上限只取前 README_MAX_BYTES 字节(按 UTF-8 边界截断)
    let meta = std::fs::metadata(&file)?;
    let content = if meta.len() > README_MAX_BYTES {
        let bytes = std::fs::read(&file)?;
        // 按 UTF-8 边界截断:跳过 continuation byte(0b10xxxxxx)
        let mut end = README_MAX_BYTES as usize;
        while end > 0 && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    } else {
        std::fs::read_to_string(&file)?
    };
    Ok(Some(content))
}

/// 列出项目内某目录的直接子项(文件树逐层懒加载;dir 为 None/空串时列根层)。
/// 含隐藏文件与 node_modules,仅跳过 .git;文件与目录都返回(空目录可见),
/// ignored 标记是否被 .gitignore / .ignore 排除(前端灰显用);结果按路径排序
pub(super) fn list_project_files(
    path: String,
    dir: Option<String>,
) -> AppResult<Vec<ProjectFileEntry>> {
    ensure_dir(&path)?;
    let root = Path::new(&path);
    let rel = dir.unwrap_or_default();
    // dir 必须解析到 root 内的目录:canonicalize 后比较前缀,拒绝 .. 越界与符号链接逃逸
    let root_canon = std::fs::canonicalize(root)?;
    let target = if rel.is_empty() {
        root_canon.clone()
    } else {
        std::fs::canonicalize(root.join(&rel))
            .map_err(|_| AppError::coded(ErrorCode::InvalidPath, rel.clone()))?
    };
    if !target.starts_with(&root_canon) || !target.is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, rel));
    }
    Ok(walk::dir_entries(root, Path::new(&rel))
        .into_iter()
        .map(|e| ProjectFileEntry {
            path: walk::to_slash(&e.path),
            ignored: e.ignored,
            is_dir: e.is_dir,
        })
        .collect())
}

/// 文件预览读取上限 512KB,与 README 一致;超出按 UTF-8 边界截断
pub(super) const PREVIEW_MAX_BYTES: u64 = 512 * 1024;

/// 读取项目内单个文件的预览内容。
/// root 为项目根目录,rel_path 为 list_project_files 返回的相对路径;
/// canonicalize 后必须仍位于 root 内,拒绝 `..` 越界与符号链接逃逸。
/// 二进制文件返回 text = None;文本超过 512KB 截断并置 truncated。
pub(super) fn read_file_preview(root: String, rel_path: String) -> AppResult<FilePreview> {
    ensure_dir(&root)?;
    let root_canon = std::fs::canonicalize(&root)?;
    let file = std::fs::canonicalize(root_canon.join(&rel_path))
        .map_err(|_| AppError::coded(ErrorCode::InvalidPath, rel_path.clone()))?;
    if !file.starts_with(&root_canon) || !file.is_file() {
        return Err(AppError::coded(ErrorCode::InvalidPath, rel_path));
    }
    let bytes = std::fs::read(&file)?;
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return Ok(FilePreview {
            text: None,
            truncated: false,
            token_count: None,
        });
    }
    let full_text = String::from_utf8_lossy(&bytes);
    let token_count = count_o200k_tokens(&full_text);
    let truncated = bytes.len() as u64 > PREVIEW_MAX_BYTES;
    let mut end = bytes.len().min(PREVIEW_MAX_BYTES as usize);
    // 按 UTF-8 边界截断:跳过 continuation byte(0b10xxxxxx)
    while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    Ok(FilePreview {
        text: Some(String::from_utf8_lossy(&bytes[..end]).into_owned()),
        truncated,
        token_count: Some(token_count),
    })
}
