use std::fs;
use std::path::Path;

use crate::commands::{files, walk};
use crate::error::AppResult;
use crate::path_util::to_forward_slash;

use super::paths::head_sha;
use super::types::{WikiContext, WikiFileContent, WikiManifest};

/// 文件树字符预算(≈25k token),超出按目录折叠
pub(super) const FILE_TREE_MAX_CHARS: usize = 100_000;
const README_MAX_CHARS: usize = 32_768;
const MANIFEST_MAX_BYTES: usize = 16 * 1024;
const PAGE_FILE_MAX_CHARS: usize = 65_536;
const PAGE_TOTAL_MAX_CHARS: usize = 240_000;

const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "dist",
    "build",
    "out",
    "coverage",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    "bin",
    "obj",
    "pods",
    "deriveddata",
];

const EXCLUDED_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "cargo.lock",
    "composer.lock",
    "gemfile.lock",
    "poetry.lock",
    "bun.lockb",
    ".ds_store",
    "thumbs.db",
];

const EXCLUDED_EXTS: &[&str] = &[
    "exe", "dll", "so", "dylib", "bin", "o", "a", "lib", "jar", "war", "class", "pyc", "pyo",
    "png", "jpg", "jpeg", "gif", "webp", "ico", "svg", "bmp", "avif", "mp3", "mp4", "wav", "ogg",
    "mov", "avi", "webm", "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst", "pdf",
    "woff", "woff2", "ttf", "otf", "eot", "db", "sqlite", "sqlite3", "map", "lock",
];

const MANIFEST_NAMES: &[&str] = &[
    "package.json",
    "pnpm-workspace.yaml",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "tsconfig.json",
];

/// 文件是否对理解项目有价值(walk 已按 gitignore 过滤,此处再排除产物/二进制/锁文件)
pub(super) fn is_wiki_relevant(rel: &str) -> bool {
    let path = Path::new(rel);
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy().to_lowercase();
        if EXCLUDED_DIRS.contains(&s.as_str()) {
            return false;
        }
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_lowercase();
    if EXCLUDED_FILE_NAMES.contains(&lower.as_str()) {
        return false;
    }
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        return false;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if EXCLUDED_EXTS.contains(&ext.to_lowercase().as_str()) {
            return false;
        }
    }
    true
}

/// 渲染文件树。超预算时优先折叠最深且含多个文件的父目录。
pub(super) fn render_file_tree(paths: &[String]) -> (String, bool) {
    if paths.join("\n").len() <= FILE_TREE_MAX_CHARS {
        return (paths.join("\n"), false);
    }
    let mut lines: Vec<String> = paths.to_vec();
    loop {
        if lines.join("\n").len() <= FILE_TREE_MAX_CHARS {
            return (lines.join("\n"), true);
        }
        let mut best: Option<(usize, usize, usize)> = None;
        let mut i = 0;
        while i < lines.len() {
            let Some(pos) = lines[i].rfind('/') else {
                i += 1;
                continue;
            };
            let parent = &lines[i][..pos];
            let start = i;
            while i < lines.len()
                && lines[i]
                    .rfind('/')
                    .is_some_and(|p| &lines[i][..p] == parent)
            {
                i += 1;
            }
            let len = i - start;
            if len >= 2 {
                let depth = parent.matches('/').count();
                if best.is_none_or(|(d, _, _)| depth > d) {
                    best = Some((depth, start, len));
                }
            }
        }
        let Some((_, start, len)) = best else {
            let mut kept = Vec::new();
            let mut used = 0;
            for line in &lines {
                if used + line.len() + 1 > FILE_TREE_MAX_CHARS {
                    break;
                }
                used += line.len() + 1;
                kept.push(line.clone());
            }
            kept.push(format!("... ({} more files)", lines.len() - kept.len()));
            return (kept.join("\n"), true);
        };
        let parent = lines[start][..lines[start].rfind('/').unwrap()].to_string();
        lines.splice(start..start + len, [format!("{parent}/ ({len} files)")]);
    }
}

/// 按 UTF-8 边界截断到 max 字节(跳过 continuation byte)
fn truncate_utf8(bytes: &[u8], max: usize) -> String {
    let mut end = bytes.len().min(max);
    while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub(super) fn read_manifests(root: &Path) -> Vec<WikiManifest> {
    MANIFEST_NAMES
        .iter()
        .filter_map(|name| {
            let file = root.join(name);
            if !file.is_file() {
                return None;
            }
            let bytes = fs::read(&file).ok()?;
            Some(WikiManifest {
                path: (*name).to_string(),
                content: truncate_utf8(&bytes, MANIFEST_MAX_BYTES),
            })
        })
        .collect()
}

pub(crate) fn collect_wiki_context(project_path: String) -> AppResult<WikiContext> {
    files::ensure_dir(&project_path)?;
    let root = Path::new(&project_path);
    let paths: Vec<String> = walk::project_files_cached(root)
        .iter()
        .map(|p| walk::to_slash(p))
        .filter(|p| is_wiki_relevant(p))
        .collect();
    let file_count = paths.len();
    let (file_tree, tree_truncated) = render_file_tree(&paths);
    let readme = files::read_readme(&project_path)?.map(|content| {
        if content.len() > README_MAX_CHARS {
            format!(
                "{}\n\n...(truncated)",
                truncate_utf8(content.as_bytes(), README_MAX_CHARS)
            )
        } else {
            content
        }
    });

    Ok(WikiContext {
        file_tree,
        paths,
        file_count,
        tree_truncated,
        readme,
        manifests: read_manifests(root),
        head_sha: head_sha(&project_path),
    })
}

/// 读取单页生成所需的相关文件全文，并拒绝路径逃逸和二进制文件。
pub(crate) fn read_wiki_files_in(
    project_path: &str,
    rel_paths: &[String],
) -> AppResult<Vec<WikiFileContent>> {
    files::ensure_dir(project_path)?;
    let root_canon = fs::canonicalize(project_path)?;
    let mut out = Vec::new();
    let mut budget = PAGE_TOTAL_MAX_CHARS;
    for rel in rel_paths {
        let Ok(file) = fs::canonicalize(root_canon.join(rel)) else {
            continue;
        };
        if !file.starts_with(&root_canon) || !file.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(&file) else {
            continue;
        };
        if bytes[..bytes.len().min(8_000)].contains(&0) {
            continue;
        }
        let cap = PAGE_FILE_MAX_CHARS.min(budget);
        let content = truncate_utf8(&bytes, cap);
        let truncated = bytes.len() > content.len();
        budget = budget.saturating_sub(content.len());
        out.push(WikiFileContent {
            path: to_forward_slash(Path::new(rel)),
            content,
            truncated,
        });
        if budget == 0 {
            break;
        }
    }
    Ok(out)
}
