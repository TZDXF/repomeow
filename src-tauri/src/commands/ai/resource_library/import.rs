//! 技能导入:本地文件夹 / zip 压缩包 / URL 三种来源。
//!
//! 「一个技能」的定义:包含 SKILL.md 的目录,SKILL.md frontmatter 的
//! `name` 为技能名称事实源。压缩包与文件夹都递归扫描 SKILL.md——单技能
//! 目录与 GitHub 整仓 zip(多技能)均可导入;每次导入有大小 / 深度 / 数量
//! 上限,跳过符号链接与路径穿越条目(zip-slip),重名或缺 name 的条目跳过。

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::errors::{codes, RlError, RlResult};
use super::frontmatter as fm;
use super::git;
use super::models::{Skill, SkillImportOutcome, SkillImportSkip, SkillLibrary};
use super::ops::new_id;
use super::store::{remove_dir_tolerating_readonly, Library, DIR_SKILLS, FILE_SKILLS};
use crate::time_util::{now_ts, now_ts_nanos};

/// 归档文件下载/读取上限;解压后总字节另计
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;
/// 解压后总字节上限(防 zip 炸弹)
const MAX_EXTRACT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// SKILL.md 扫描深度上限
const MAX_SCAN_DEPTH: usize = 8;
/// 单次导入的技能数量上限(超出部分静默停止扫描)
const MAX_SKILLS_PER_IMPORT: usize = 50;

/// 递归收集包含 SKILL.md 的技能根目录;命中 SKILL.md 的目录视为一个技能,
/// 不再向更深层递归。跳过符号链接(symlink_metadata 的 is_dir 为 false)。
fn collect_skill_roots(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_SCAN_DEPTH || out.len() >= MAX_SKILLS_PER_IMPORT {
        return;
    }
    if dir.join("SKILL.md").is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() {
            collect_skill_roots(&entry.path(), out, depth + 1);
        }
    }
}

/// 递归复制技能目录内容到库内(skills/<id>/);跳过符号链接
fn copy_dir_recursive(src: &Path, dest: &Path) -> RlResult<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)?.flatten() {
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let target = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn skip(name: &str, reason: &str) -> SkillImportSkip {
    SkillImportSkip {
        name: name.to_string(),
        reason: reason.to_string(),
    }
}

/// 把收集到的技能根目录逐个导入;部分成功语义,全部处理完才写回 skills.json
/// 并做一次快照提交(成功导入非空时)。
fn import_from_roots(lib: &Library, roots: &[PathBuf]) -> RlResult<SkillImportOutcome> {
    lib.ensure()?;
    let mut data: SkillLibrary = lib.read_plain_json(FILE_SKILLS)?;
    let mut outcome = SkillImportOutcome::default();
    let mut sort_order = data
        .skills
        .iter()
        .map(|s| s.sort_order)
        .max()
        .map_or(0, |m| m + 1);
    for root in roots {
        let dir_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let body = match fs::read(root.join("SKILL.md"))
            .map_err(|e| RlError::coded(codes::SKILL_IMPORT_EMPTY, e.to_string()))
            .and_then(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|e| RlError::coded(codes::SKILL_IMPORT_EMPTY, e.to_string()))
            }) {
            Ok(body) => body,
            Err(_) => {
                outcome.skipped.push(skip(&dir_name, "invalid"));
                continue;
            }
        };
        let (name, description) = fm::name_description_of(&body);
        let Some(name) = name.filter(|value| !value.trim().is_empty()) else {
            outcome.skipped.push(skip(&dir_name, "invalid"));
            continue;
        };
        if data.skills.iter().any(|s| s.name == name) {
            outcome.skipped.push(skip(&name, "conflict"));
            continue;
        }
        let id = new_id("sk");
        copy_dir_recursive(root, &lib.root().join(DIR_SKILLS).join(&id))?;
        let ts = now_ts();
        let skill = Skill {
            id: id.clone(),
            directory: id,
            name: name.clone(),
            description: description.unwrap_or_default(),
            marketplace: None,
            group_ids: Vec::new(),
            sort_order,
            created_at: ts,
            updated_at: ts,
        };
        sort_order += 1;
        data.skills.push(skill.clone());
        outcome.imported.push(skill);
    }
    if !outcome.imported.is_empty() {
        let message = if outcome.imported.len() == 1 {
            format!("导入技能:{}", outcome.imported[0].name)
        } else {
            format!(
                "导入技能:{} 等 {} 个",
                outcome.imported[0].name,
                outcome.imported.len()
            )
        };
        lib.write_plain_json(FILE_SKILLS, &data)?;
        git::auto_commit(lib, &message)?;
    }
    Ok(outcome)
}

/// 从本地文件夹导入:文件夹本身或其子目录中包含 SKILL.md 的目录均算技能
pub(super) fn skill_import_folder(lib: &Library, path: &str) -> RlResult<SkillImportOutcome> {
    let root = PathBuf::from(path.trim());
    if root.as_os_str().is_empty() || !root.is_dir() {
        return Err(RlError::coded(codes::IMPORT_SOURCE_INVALID, path));
    }
    let mut roots = Vec::new();
    collect_skill_roots(&root, &mut roots, 0);
    if roots.is_empty() {
        return Err(RlError::coded(codes::SKILL_IMPORT_EMPTY, path));
    }
    import_from_roots(lib, &roots)
}

/// 把 zip 字节安全解压到临时目录;返回临时目录(调用方负责清理)。
/// 跳过符号链接与路径穿越条目(enclosed_name 为 None),条目/总量超限报错。
fn extract_zip_to_temp(bytes: &[u8]) -> RlResult<PathBuf> {
    let temp = std::env::temp_dir().join(format!(
        "repomeow-skill-import-{}-{}",
        std::process::id(),
        now_ts_nanos()
    ));
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| RlError::coded(codes::ARCHIVE_INVALID, e.to_string()))?;
    fs::create_dir_all(&temp)?;
    let result = (|| -> RlResult<()> {
        let mut total: u64 = 0;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| RlError::coded(codes::ARCHIVE_INVALID, e.to_string()))?;
            if entry.is_dir() {
                continue;
            }
            // 符号链接与绝对/穿越路径一律跳过(zip-slip 防护)
            let is_symlink = entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170_000 == 0o120_000);
            if is_symlink {
                continue;
            }
            let Some(rel) = entry.enclosed_name() else {
                continue;
            };
            total = total.saturating_add(entry.size());
            if total > MAX_EXTRACT_TOTAL_BYTES {
                return Err(RlError::coded(codes::ARCHIVE_TOO_LARGE, "extracted"));
            }
            let target = temp.join(rel);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = fs::File::create(&target)?;
            if std::io::copy(&mut entry, &mut out).is_err() {
                return Err(RlError::coded(
                    codes::ARCHIVE_INVALID,
                    format!("entry {index}"),
                ));
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        remove_dir_tolerating_readonly(&temp)?;
        return Err(e);
    }
    Ok(temp)
}

fn import_archive_bytes(lib: &Library, bytes: &[u8]) -> RlResult<SkillImportOutcome> {
    let temp = extract_zip_to_temp(bytes)?;
    let result = (|| {
        let mut roots = Vec::new();
        collect_skill_roots(&temp, &mut roots, 0);
        if roots.is_empty() {
            return Err(RlError::coded(codes::SKILL_IMPORT_EMPTY, ""));
        }
        import_from_roots(lib, &roots)
    })();
    remove_dir_tolerating_readonly(&temp)?;
    result
}

/// 从本地 zip 压缩包导入
pub(super) fn skill_import_archive(lib: &Library, path: &str) -> RlResult<SkillImportOutcome> {
    let path = path.trim();
    let meta = fs::metadata(path)
        .map_err(|e| RlError::coded(codes::IMPORT_SOURCE_INVALID, e.to_string()))?;
    if !meta.is_file() {
        return Err(RlError::coded(codes::IMPORT_SOURCE_INVALID, path));
    }
    if meta.len() > MAX_ARCHIVE_BYTES {
        return Err(RlError::coded(codes::ARCHIVE_TOO_LARGE, path));
    }
    let bytes = fs::read(path)?;
    import_archive_bytes(lib, &bytes)
}

fn download_limited(url: &str) -> RlResult<Vec<u8>> {
    let response = reqwest::blocking::Client::builder()
        .user_agent("RepoMeow resource library")
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| RlError::coded(codes::DOWNLOAD_FAILED, e.to_string()))?
        .get(url)
        .send()
        .map_err(|e| RlError::coded(codes::DOWNLOAD_FAILED, e.to_string()))?;
    if !response.status().is_success() {
        return Err(RlError::coded(
            codes::DOWNLOAD_FAILED,
            format!("{} {url}", response.status()),
        ));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_ARCHIVE_BYTES)
    {
        return Err(RlError::coded(codes::ARCHIVE_TOO_LARGE, "content-length"));
    }
    let mut out = Vec::new();
    response
        .take(MAX_ARCHIVE_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|e| RlError::coded(codes::DOWNLOAD_FAILED, e.to_string()))?;
    if out.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(RlError::coded(codes::ARCHIVE_TOO_LARGE, "body"));
    }
    Ok(out)
}

/// 从 URL 下载 zip 压缩包导入(仅 http/https)
pub(super) fn skill_import_url(lib: &Library, url: &str) -> RlResult<SkillImportOutcome> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(RlError::coded(codes::URL_INVALID, url));
    }
    let bytes = download_limited(url)?;
    import_archive_bytes(lib, &bytes)
}
