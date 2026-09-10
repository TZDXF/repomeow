//! 技能导入:本地文件夹 / zip 压缩包 / Git 仓库 URL 三种来源。
//!
//! 「一个技能」的定义:包含 SKILL.md 的目录,SKILL.md frontmatter 的
//! `name` 为技能名称事实源。压缩包与文件夹都递归扫描 SKILL.md——单技能
//! 目录与 GitHub 整仓 zip(多技能)均可导入;每次导入有大小 / 深度 / 数量
//! 上限,跳过符号链接与路径穿越条目(zip-slip),重名或缺 name 的条目跳过。
//! Git 克隆有整体超时与工作区体量上限,防止巨型/缓慢仓库长期占用
//! 线程与资源库全局锁。

use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::errors::{codes, RlError, RlResult};
use super::frontmatter as fm;
use super::git;
use super::models::{Skill, SkillImportOutcome, SkillImportSkip, SkillLibrary};
use super::ops::{new_id, pick_skill_directory};
use super::store::{remove_dir_tolerating_readonly, Library, DIR_SKILLS, FILE_SKILLS};
use crate::commands::git::{friendly_git_error, git_command};
use crate::error::{AppError, ErrorCode};
use crate::time_util::{now_ts, now_ts_nanos};

/// 归档文件下载/读取上限;解压后总字节另计
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;
/// 解压后总字节上限(防 zip 炸弹)
const MAX_EXTRACT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// SKILL.md 扫描深度上限
const MAX_SCAN_DEPTH: usize = 8;
/// 单次导入的技能数量上限(超出部分静默停止扫描)
const MAX_SKILLS_PER_IMPORT: usize = 50;
/// Git 克隆整体超时(与历史 zip 下载超时一致):缓慢/巨型仓库不得长期占用
/// spawn_blocking 线程与资源库全局锁
const CLONE_TIMEOUT: Duration = Duration::from_secs(120);
/// 克隆工作区总字节上限(与 zip 解压上限同口径,克隆完成后统计,不含 .git)
const MAX_CLONE_BYTES: u64 = MAX_EXTRACT_TOTAL_BYTES;

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
    let mut taken: HashSet<String> = data.skills.iter().map(|s| s.directory.clone()).collect();
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
        let directory = pick_skill_directory(&name, &taken);
        taken.insert(directory.clone());
        copy_dir_recursive(root, &lib.root().join(DIR_SKILLS).join(&directory))?;
        let ts = now_ts();
        let skill = Skill {
            id: id.clone(),
            directory,
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
        // 尽力清理:失败不掩盖真正的解压错误(Windows 杀软/索引锁常见)
        let _ = remove_dir_tolerating_readonly(&temp);
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
    // 尽力清理:清理失败不掩盖导入结果,残留临时目录交由系统清理
    let _ = remove_dir_tolerating_readonly(&temp);
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

/// 终止克隆子进程;Windows 上 clone 会派生 remote helper 孙进程,
/// 用 taskkill /T 杀整棵进程树(与项目克隆取消同策略)
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let pid = child.id();
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
    }
    // 非 Windows 主路径;Windows 上作为 taskkill 的兜底(重复 kill 无害)
    let _ = child.kill();
}

/// 浅克隆 url 到 target,带整体超时:超时杀掉整棵进程树并报 CLONE_TIMEOUT;
/// 非零退出复用 git 层的友好错误映射(与 run_git 语义一致)。
/// stderr 由独立线程持续消费:clone 进度行刷在 stderr,管道写满会阻塞子进程。
fn clone_repo(workdir: &Path, url: &str, target: &Path, timeout: Duration) -> RlResult<()> {
    let dir = workdir.to_string_lossy().into_owned();
    let target_str = target.to_string_lossy().into_owned();
    let mut child = git_command(&dir)
        .args(["clone", "--depth", "1", "--", url, &target_str])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RlError::Io)?;
    let drainer = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_tree(&mut child);
                    let _ = child.wait();
                    return Err(RlError::coded(codes::CLONE_TIMEOUT, url));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                kill_process_tree(&mut child);
                let _ = child.wait();
                return Err(RlError::Io(e));
            }
        }
    };
    if status.success() {
        return Ok(());
    }
    let stderr = drainer.and_then(|t| t.join().ok()).unwrap_or_default();
    let detail = String::from_utf8_lossy(&stderr).trim().to_string();
    Err(RlError::App(if detail.is_empty() {
        AppError::coded(
            ErrorCode::GitCommandFailed,
            format!("clone status={status}"),
        )
    } else {
        friendly_git_error(&detail)
    }))
}

/// 目录总字节数(跳过符号链接,与 copy/扫描口径一致)
fn dir_total_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// 浅克隆 Git 仓库到临时目录并导入(与 URL 校验分离,测试可用本地仓库路径)。
/// 克隆带超时与工作区体量上限;完成后移除 .git(无导入价值且拖慢 SKILL.md
/// 扫描),无论成败都尽力清理临时目录(清理失败不掩盖导入结果)。
pub(super) fn clone_and_import(lib: &Library, url: &str) -> RlResult<SkillImportOutcome> {
    clone_and_import_with_limits(lib, url, CLONE_TIMEOUT, MAX_CLONE_BYTES)
}

pub(super) fn clone_and_import_with_limits(
    lib: &Library,
    url: &str,
    timeout: Duration,
    max_bytes: u64,
) -> RlResult<SkillImportOutcome> {
    let parent = std::env::temp_dir();
    let temp = parent.join(format!(
        "repomeow-skill-clone-{}-{}",
        std::process::id(),
        now_ts_nanos()
    ));
    let result = (|| -> RlResult<SkillImportOutcome> {
        clone_repo(&parent, url, &temp, timeout)?;
        let git_dir = temp.join(".git");
        if git_dir.exists() {
            remove_dir_tolerating_readonly(&git_dir)?;
        }
        if dir_total_bytes(&temp) > max_bytes {
            return Err(RlError::coded(codes::REPO_TOO_LARGE, url));
        }
        let mut roots = Vec::new();
        collect_skill_roots(&temp, &mut roots, 0);
        if roots.is_empty() {
            return Err(RlError::coded(codes::SKILL_IMPORT_EMPTY, url));
        }
        import_from_roots(lib, &roots)
    })();
    // 尽力清理:Windows 杀软/索引器占用导致删除失败时不得把已成功的导入报错
    let _ = remove_dir_tolerating_readonly(&temp);
    result
}

/// 从 Git 仓库 URL(如 GitHub 仓库地址)克隆代码并导入其中的技能(仅 http/https)
pub(super) fn skill_import_url(lib: &Library, url: &str) -> RlResult<SkillImportOutcome> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(RlError::coded(codes::URL_INVALID, url));
    }
    clone_and_import(lib, url)
}
