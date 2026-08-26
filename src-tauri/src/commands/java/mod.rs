mod builds;
mod detection;
mod install;
mod remote;

use tauri::AppHandle;

use crate::error::AppResult;
use crate::models::{JdkCandidate, JdkVendor, RemoteJdkRelease};

pub(crate) use builds::java_builds_from_files;

/// 自动探测本机 JDK 候选(设置页「自动探测」按钮调用)。
#[tauri::command]
pub async fn detect_jdks() -> AppResult<Vec<JdkCandidate>> {
    detection::detect_jdks().await
}

/// 校验手动添加的 JDK 根目录。
#[tauri::command]
pub fn check_jdk(path: String) -> AppResult<String> {
    detection::check_jdk(path)
}

/// 列出某安装源可在线安装的 JDK 大版本。
#[tauri::command]
pub async fn list_remote_jdks(vendor: JdkVendor) -> AppResult<Vec<RemoteJdkRelease>> {
    remote::list_remote_jdks(vendor).await
}

/// 在线下载并安装 JDK 到 ~/.jdks。
#[tauri::command]
pub async fn install_jdk(app: AppHandle, vendor: JdkVendor, major: u32) -> AppResult<JdkCandidate> {
    install::install_jdk(app, vendor, major).await
}

#[cfg(test)]
mod tests;
