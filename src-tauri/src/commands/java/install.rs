use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter};

use super::detection::{display_path, env_dir, java_bin, probe_java_version};
use super::remote::{http_client, resolve_asset};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{JdkCandidate, JdkVendor};

/// 安装进度事件(install_jdk 向前端 emit),stage = "download" | "extract"
const JDK_PROGRESS_EVENT: &str = "jdk://install-progress";
/// 下载进度事件的最小发射间隔(字节):约 2MB 一次,避免高频 IPC 刷屏
const PROGRESS_EMIT_BYTES: u64 = 2 * 1024 * 1024;

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
fn download_with_progress(app: &AppHandle, url: &str, dest: &Path) -> AppResult<()> {
    // 不设总超时:近 200MB 的包在慢网下远超常规超时,连接超时已兜住坏网络
    let client = http_client(None)?;
    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("{url}: {e}")))?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut received = 0u64;
    let mut emitted = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
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
    let _ = app.emit(
        JDK_PROGRESS_EVENT,
        serde_json::json!({ "stage": "download", "received": received, "total": total.max(received) }),
    );
    Ok(())
}

/// 解压 zip 到目标目录(enclosed_name 拒绝 zip-slip 路径穿越;Unix 上恢复可执行位)
pub(super) fn extract_zip(archive: &Path, dest: &Path) -> AppResult<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("zip: {e}")))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| AppError::coded(ErrorCode::JdkInstallFailed, format!("zip: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut outfile = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut outfile)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// 非 Windows 的 Adoptium tar.gz 用系统 tar 解(bsdtar/gnu tar 均支持 -xzf)
fn extract_tar_gz(archive: &Path, dest: &Path) -> AppResult<()> {
    let out = crate::commands::open::hidden(std::process::Command::new("tar"))
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .output()?;
    if !out.status.success() {
        return Err(AppError::coded(
            ErrorCode::JdkInstallFailed,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
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
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
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
    let jdk_root = jdks_root()?;
    let tmp_file = std::env::temp_dir().join(format!(
        "repomeow-jdk-{}",
        asset.file_name.replace(['/', '\\'], "_")
    ));
    if let Err(e) = download_with_progress(app, &asset.url, &tmp_file) {
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
