use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter};
use sha2::{Digest, Sha256};

use super::detection::{display_path, env_dir, java_bin, probe_java_version};
use super::remote::{asset_checksum, http_client, resolve_asset};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{JdkCandidate, JdkVendor};

/// 安装进度事件(install_jdk 向前端 emit),stage = "download" | "extract"
const JDK_PROGRESS_EVENT: &str = "jdk://install-progress";
/// 下载进度事件的最小发射间隔(字节):约 2MB 一次,避免高频 IPC 刷屏
const PROGRESS_EMIT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 100_000;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 安装根目录 ~/.jdks(与 IntelliJ 下载 JDK 的目录一致,自动探测也覆盖该目录)
fn jdks_root() -> AppResult<PathBuf> {
    let home = env_dir("USERPROFILE")
        .or_else(|| env_dir("HOME"))
        .ok_or_else(|| AppError::coded(ErrorCode::JdkInstallFailed, "no user home dir"))?;
    let root = home.join(".jdks");
    std::fs::create_dir_all(&root)?;
    Ok(root)
}

/// 下载发行包到临时文件,期间按字节量节流 emit 下载进度
fn download_with_progress(app: &AppHandle, url: &str, dest: &Path, checksum: &str) -> AppResult<()> {
    // 不设总超时:近 200MB 的包在慢网下远超常规超时,连接超时已兜住坏网络
    let client = http_client(None)?;
    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("{url}: {e}")))?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut emitted = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        received += n as u64;
        if received - emitted >= PROGRESS_EMIT_BYTES {
            emitted = received;
            let _ = app.emit(
                JDK_PROGRESS_EVENT,
                serde_json::json!({ "stage": "download", "received": received, "total": total }),
            );
        }
    }
    file.flush()?;
    verify_checksum(hasher, checksum)?;
    let _ = app.emit(
        JDK_PROGRESS_EVENT,
        serde_json::json!({ "stage": "download", "received": received, "total": total.max(received) }),
    );
    Ok(())
}

fn verify_checksum(hasher: Sha256, expected: &str) -> AppResult<()> {
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(AppError::coded(ErrorCode::JdkInstallFailed, "SHA-256 checksum mismatch"));
    }
    Ok(())
}

/// 解压 zip 到目标目录(enclosed_name 拒绝 zip-slip 路径穿越;Unix 上恢复可执行位)
pub(super) fn extract_zip(archive: &Path, dest: &Path) -> AppResult<()> {
    extract_zip_with_limits(archive, dest, MAX_ZIP_ENTRIES, MAX_EXTRACTED_BYTES)
}

fn extract_zip_with_limits(archive: &Path, dest: &Path, max_entries: usize, max_bytes: u64) -> AppResult<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("zip: {e}")))?;
    if zip.len() > max_entries {
        return Err(AppError::coded(ErrorCode::JdkInstallFailed, "zip 文件条目过多"));
    }
    let mut extracted = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("zip: {e}")))?;
        let rel = entry.enclosed_name().ok_or_else(||
            AppError::coded(ErrorCode::JdkInstallFailed, "zip 包含不安全路径"))?;
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "zip 包含符号链接"));
        }
        if entry.size() > max_bytes.saturating_sub(extracted) {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "zip 解压大小超过限制"));
        }
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        let mode = entry.unix_mode();
        let mut outfile = std::fs::File::create(&out)?;
        let remaining = max_bytes - extracted;
        let copied = std::io::copy(&mut entry.take(remaining + 1), &mut outfile)?;
        if copied > remaining {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "zip 解压大小超过限制"));
        }
        extracted += copied;
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// 在应用内有界解压 tar.gz；符号链接延后创建，硬链接和特殊文件一律拒绝。
fn extract_tar_gz(archive: &Path, dest: &Path) -> AppResult<()> {
    extract_tar_gz_with_limits(archive, dest, MAX_ZIP_ENTRIES, MAX_EXTRACTED_BYTES)
}

fn extract_tar_gz_with_limits(archive: &Path, dest: &Path, max_entries: usize, max_bytes: u64) -> AppResult<()> {
    // 只在空目录中解压，保证归档读取期间不会经过已有符号链接。
    std::fs::create_dir_all(dest)?;
    if !std::fs::symlink_metadata(dest)?.file_type().is_dir()
        || std::fs::read_dir(dest)?.next().is_some()
    {
        return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 解压目录必须为空且不是符号链接"));
    }
    let file = std::fs::File::open(archive)?;
    // 同时限制 tar 的头部及扩展元数据，避免大体积 PAX/长文件名绕过文件大小限制。
    let stream_limit = max_bytes.saturating_add(256 * 1024 * 1024);
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder.take(stream_limit.saturating_add(1)));
    let mut count = 0usize;
    let mut total = 0u64;
    let mut links: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in tar.entries()? {
        count += 1;
        if count > max_entries {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 文件条目过多"));
        }
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.as_os_str().is_empty() || !path.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir)) {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 包含不安全路径"));
        }
        let path: PathBuf = path.components().filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        }).collect();
        if path.as_os_str().is_empty() {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 包含空路径"));
        }
        let kind = entry.header().entry_type();
        if kind.is_symlink() {
            let target = entry.link_name()?.ok_or_else(||
                AppError::coded(ErrorCode::JdkInstallFailed, "tar 符号链接缺少目标"))?;
            links.push((path, target.into_owned()));
            continue;
        }
        let out = dest.join(path);
        if kind.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if !kind.is_file() {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 包含链接或特殊文件"));
        }
        let size = entry.size();
        if size > max_bytes.saturating_sub(total) {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 解压大小超过限制"));
        }
        if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
        #[cfg(unix)]
        let mode = entry.header().mode()? & 0o777;
        let mut outfile = std::fs::OpenOptions::new().write(true).create_new(true).open(&out)?;
        let copied = std::io::copy(&mut entry, &mut outfile)?;
        if copied != size {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "tar 文件长度不匹配"));
        }
        total += copied;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))?;
        }
    }
    // 先完整验证整个链接集合，再创建链接；不得通过归档中其他链接间接解析。
    validate_tar_links(&links)?;
    #[cfg(unix)]
    for (path, target) in &links {
        let out = dest.join(path);
        let parent = out.parent().ok_or_else(||
            AppError::coded(ErrorCode::JdkInstallFailed, "tar 符号链接路径无父目录"))?;
        std::fs::create_dir_all(parent)?;
        std::os::unix::fs::symlink(target, out)?;
    }
    #[cfg(not(unix))]
    if !links.is_empty() {
        return Err(AppError::coded(ErrorCode::JdkInstallFailed, "当前平台不支持 tar 符号链接"));
    }
    Ok(())
}

/// 拒绝绝对链接、越出 JDK 顶层目录的链接，以及链接链或位于链接下的路径。
fn validate_tar_links(links: &[(PathBuf, PathBuf)]) -> AppResult<()> {
    use std::collections::HashSet;
    use std::path::Component;
    let invalid = || AppError::coded(ErrorCode::JdkInstallFailed, "tar 包含不安全的符号链接");
    let paths: HashSet<&Path> = links.iter().map(|(path, _)| path.as_path()).collect();
    if paths.len() != links.len() { return Err(invalid()); }
    for (path, target) in links {
        let top = match path.components().next() {
            Some(Component::Normal(name)) => name,
            _ => return Err(invalid()),
        };
        let mut resolved = PathBuf::new();
        // 符号链接本身不得位于另一个符号链接下。
        for component in path.parent().ok_or_else(invalid)?.components() {
            if let Component::Normal(name) = component { resolved.push(name); }
            if paths.contains(resolved.as_path()) { return Err(invalid()); }
        }
        if resolved.as_os_str().is_empty() || target.as_os_str().is_empty() { return Err(invalid()); }
        for component in target.components() {
            match component {
                Component::Normal(name) => {
                    resolved.push(name);
                    if paths.contains(resolved.as_path()) { return Err(invalid()); }
                }
                Component::CurDir => {},
                Component::ParentDir => {
                    if resolved.components().count() <= 1 { return Err(invalid()); }
                    resolved.pop();
                }
                _ => return Err(invalid()),
            }
        }
        if resolved.components().next() != Some(Component::Normal(top)) { return Err(invalid()); }
    }
    Ok(())
}

/// 递归复制目录(rename 跨卷失败时的兜底,如临时目录与用户目录不在同一盘)
fn copy_dir_recursive(src: &Path, dest: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
            #[cfg(not(unix))]
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "当前平台不支持复制符号链接"));
        } else if kind.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if kind.is_file() {
            std::fs::copy(&from, &to)?;
        } else {
            return Err(AppError::coded(ErrorCode::JdkInstallFailed, "JDK 包含特殊文件"));
        }
    }
    Ok(())
}

pub(super) fn move_dir(src: &Path, dest: &Path) -> AppResult<()> {
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    copy_dir_recursive(src, dest)?;
    let _ = std::fs::remove_dir_all(src);
    Ok(())
}

/// install_jdk 的阻塞实现:解析资产 -> 下载(进度事件)-> 解压 -> 挪进 ~/.jdks -> 校验。
/// 目标目录已是有效 JDK 时幂等返回(重复点击安装不报错)
fn install_jdk_blocking(app: &AppHandle, vendor: JdkVendor, major: u32) -> AppResult<JdkCandidate> {
    let asset = resolve_asset(vendor, major)?;
    let checksum = asset_checksum(&asset)?;
    let jdk_root = jdks_root()?;
    let tmp_file = std::env::temp_dir().join(format!(
        "repomeow-jdk-{}-{}-{}",
        std::process::id(), crate::time_util::now_ts_nanos(),
        asset.file_name.replace(['/', '\\'], "_")
    ));
    if let Err(e) = download_with_progress(app, &asset.url, &tmp_file, &checksum) {
        let _ = std::fs::remove_file(&tmp_file);
        return Err(e);
    }
    let extract_dir = std::env::temp_dir().join(format!(
        "repomeow-jdk-extract-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos(),
    ));
    let extracted = (|| -> AppResult<PathBuf> {
        std::fs::create_dir_all(&extract_dir)?;
        let _ = app.emit(
            JDK_PROGRESS_EVENT,
            serde_json::json!({ "stage": "extract", "received": 0, "total": 0 }),
        );
        if asset.is_zip {
            extract_zip(&tmp_file, &extract_dir)?;
        } else {
            extract_tar_gz(&tmp_file, &extract_dir)?;
        }
        // 发行包标准布局:解压出来唯一一个顶层目录即 JDK 根
        let tops: Vec<PathBuf> = std::fs::read_dir(&extract_dir)?
            .flatten()
            .map(|e| e.path())
            .collect();
        if tops.len() != 1 || !tops[0].is_dir() {
            return Err(AppError::coded(
                ErrorCode::JdkInstallFailed,
                format!("unexpected archive layout in {}", tops.len()),
            ));
        }
        Ok(tops[0].clone())
    })();
    let _ = std::fs::remove_file(&tmp_file);
    let top = match extracted {
        Ok(top) => top,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&extract_dir);
            return Err(e);
        }
    };

    let target = jdk_root.join(top.file_name().unwrap_or_default());
    let result = (|| -> AppResult<JdkCandidate> {
        // 幂等:目标已存在且是有效 JDK 直接返回现有安装
        if java_bin(&target).is_some() {
            let version =
                probe_java_version(&java_bin(&target).expect("checked")).ok_or_else(|| {
                    AppError::coded(ErrorCode::JdkInstallFailed, display_path(&target))
                })?;
            return Ok(JdkCandidate {
                path: display_path(&target),
                version,
            });
        }
        if target.exists() {
            return Err(AppError::coded(
                ErrorCode::JdkInstallFailed,
                display_path(&target),
            ));
        }
        move_dir(&top, &target)?;
        let java = java_bin(&target)
            .ok_or_else(|| AppError::coded(ErrorCode::JdkInstallFailed, display_path(&target)))?;
        let version = probe_java_version(&java)
            .ok_or_else(|| AppError::coded(ErrorCode::JdkInstallFailed, display_path(&target)))?;
        Ok(JdkCandidate {
            path: display_path(&target),
            version,
        })
    })();
    let _ = std::fs::remove_dir_all(&extract_dir);
    result
}

/// 在线下载并安装 JDK 到 ~/.jdks(install_jdk_blocking 见块注释)
pub(super) async fn install_jdk(
    app: AppHandle,
    vendor: JdkVendor,
    major: u32,
) -> AppResult<JdkCandidate> {
    tokio::task::spawn_blocking(move || install_jdk_blocking(&app, vendor, major))
        .await
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?
}

#[cfg(test)]
mod hardening_tests {
    use super::*;

    #[test]
    fn zip_rejects_entry_count_and_uncompressed_size_limits() {
        let root = std::env::temp_dir().join(format!("repomeow-zip-limits-{}-{}", std::process::id(), crate::time_util::now_ts_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("test.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        writer.start_file("jdk/a", zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(b"12345").unwrap();
        writer.start_file("jdk/b", zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(b"67890").unwrap();
        writer.finish().unwrap();
        let dest = root.join("out");
        assert!(extract_zip_with_limits(&archive, &dest, 1, 100).is_err());
        assert!(extract_zip_with_limits(&archive, &dest, 2, 9).is_err());
        assert!(extract_zip_with_limits(&archive, &dest, 2, 10).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn checksum_accepts_correct_digest_and_rejects_mismatch() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert!(verify_checksum(hasher.clone(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").is_ok());
        assert!(verify_checksum(hasher, &"0".repeat(64)).is_err());
    }

    #[test]
    fn tar_enforces_count_size_and_rejects_links() {
        let root = std::env::temp_dir().join(format!("repomeow-tar-limits-{}-{}", std::process::id(), crate::time_util::now_ts_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("test.tar.gz");
        let encoder = flate2::write::GzEncoder::new(std::fs::File::create(&archive).unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for path in ["jdk/a", "jdk/b"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, &b"12345"[..]).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        assert!(extract_tar_gz_with_limits(&archive, &root.join("count"), 1, 20).is_err());
        assert!(extract_tar_gz_with_limits(&archive, &root.join("size"), 2, 9).is_err());
        assert!(extract_tar_gz_with_limits(&archive, &root.join("ok"), 2, 10).is_ok());

        let link_archive = root.join("link.tar.gz");
        let encoder = flate2::write::GzEncoder::new(std::fs::File::create(&link_archive).unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name("../../escape").unwrap();
        header.set_cksum();
        builder.append_data(&mut header, "jdk/link", std::io::empty()).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert!(extract_tar_gz_with_limits(&link_archive, &root.join("link"), 2, 10).is_err());

        let traversal_archive = root.join("traversal.tar.gz");
        let encoder = flate2::write::GzEncoder::new(std::fs::File::create(&traversal_archive).unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        let name = b"../escape";
        header.as_mut_bytes()[..name.len()].copy_from_slice(name);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, std::io::empty()).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert!(extract_tar_gz_with_limits(&traversal_archive, &root.join("traversal"), 2, 10).is_err());
        assert!(!root.join("escape").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlink_validation_confines_targets_and_rejects_link_chains() {
        fn links(items: &[(&str, &str)]) -> Vec<(PathBuf, PathBuf)> {
            items.iter().map(|(p, t)| (PathBuf::from(p), PathBuf::from(t))).collect()
        }
        assert!(validate_tar_links(&links(&[("jdk/bin/tool", "../lib/tool")])).is_ok());
        assert!(validate_tar_links(&links(&[("jdk/bin/tool", "../../outside")])).is_err());
        assert!(validate_tar_links(&links(&[("jdk/bin/tool", "/etc/passwd")])).is_err());
        assert!(validate_tar_links(&links(&[("jdk/a", "lib"), ("jdk/b", "a/file")])).is_err());
        assert!(validate_tar_links(&links(&[("jdk/a", "lib"), ("jdk/a/b", "file")])).is_err());
        assert!(validate_tar_links(&links(&[("jdk/a", "lib"), ("jdk/b", "a/../file")])).is_err());
        assert!(validate_tar_links(&links(&[("jdk/a", "lib"), ("jdk/a", "bin")])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn tar_extracts_safe_link_and_cross_volume_copy_preserves_it() {
        let root = std::env::temp_dir().join(format!("repomeow-tar-link-{}-{}", std::process::id(), crate::time_util::now_ts_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("test.tar.gz");
        let encoder = flate2::write::GzEncoder::new(std::fs::File::create(&archive).unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o777);
        link.set_link_name("../lib/tool").unwrap();
        link.set_cksum();
        builder.append_data(&mut link, "jdk/bin/tool", std::io::empty()).unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_size(2);
        file.set_mode(0o755);
        file.set_cksum();
        builder.append_data(&mut file, "jdk/lib/tool", &b"ok"[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let extracted = root.join("out");
        extract_tar_gz_with_limits(&archive, &extracted, 2, 100).unwrap();
        assert_eq!(std::fs::read(extracted.join("jdk/bin/tool")).unwrap(), b"ok");
        let copied = root.join("copy");
        copy_dir_recursive(&extracted.join("jdk"), &copied).unwrap();
        assert!(std::fs::symlink_metadata(copied.join("bin/tool")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(copied.join("bin/tool")).unwrap(), b"ok");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tar_does_not_write_through_deferred_link() {
        let root = std::env::temp_dir().join(format!("repomeow-tar-write-link-{}-{}", std::process::id(), crate::time_util::now_ts_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("test.tar.gz");
        let encoder = flate2::write::GzEncoder::new(std::fs::File::create(&archive).unwrap(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o777);
        link.set_link_name("../../outside").unwrap();
        link.set_cksum();
        builder.append_data(&mut link, "jdk/link", std::io::empty()).unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_size(1);
        file.set_mode(0o644);
        file.set_cksum();
        builder.append_data(&mut file, "jdk/link/escape", &b"x"[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        assert!(extract_tar_gz_with_limits(&archive, &root.join("out"), 2, 10).is_err());
        assert!(!root.join("outside/escape").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

}
