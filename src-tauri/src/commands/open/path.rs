use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;

/// 交给系统默认处理程序等于直接执行的扩展名(Windows 上 .js/.vbs/.lnk 等双击
/// 即运行),拒绝用默认程序打开;文档/图片/源码等数据类文件不受影响。项目源码
/// 应经编辑器打开(`open_in_editor`),不走这里。
const BLOCKED_OPEN_EXTENSIONS: [&str; 30] = [
    "exe", "msi", "msix", "appx", "appxbundle", "bat", "cmd", "com", "scr", "pif", "ps1", "psd1",
    "psm1", "vbs", "vbe", "js", "jse", "ws", "wsf", "wsh", "hta", "lnk", "url", "jar", "cpl",
    "msc", "reg", "chm", "application", "appref-ms",
];

/// 校验用系统默认程序打开的本地路径:拒绝空路径、`..` 穿越组件、设备路径
/// (`\\.\` / `\\?\`)与可执行/脚本类扩展名(目录不检查扩展名,目录名可含点)。
/// 取代 opener 插件的 `opener:allow-open-path` 通配授权——那是唯一把
/// "webview 内任意路径交给默认程序"升级为任意程序执行的入口。
pub(crate) fn validate_open_path(raw: &str) -> AppResult<String> {
    // 空白输入直接拒绝(clean_str 会把纯分隔符输入归一为根,不能再靠 clean 判空)
    if raw.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::InvalidPath, raw.to_string()));
    }
    let clean = clean_str(raw);
    let forward = clean.replace('\\', "/");
    if forward.split('/').any(|part| part == "..")
        || forward.starts_with("//./")
        || forward.starts_with("//?/")
    {
        return Err(AppError::coded(ErrorCode::InvalidPath, raw.to_string()));
    }
    let path = std::path::Path::new(&clean);
    if !path.is_dir() {
        let blocked = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .is_some_and(|ext| BLOCKED_OPEN_EXTENSIONS.contains(&ext.as_str()));
        if blocked {
            return Err(AppError::coded(
                ErrorCode::InvalidPath,
                "executable files cannot be opened by the system default handler",
            ));
        }
    }
    Ok(clean)
}

/// 以系统默认处理程序打开本地文件/目录(Markdown 链接、文件抽屉、技能目录)。
/// 走 Rust 侧 opener API,不经前端 capability 授权;可执行类扩展名在
/// `validate_open_path` 拒绝。
#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> AppResult<()> {
    let clean = validate_open_path(&path)?;
    app.opener()
        .open_path(&clean, None::<&str>)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))
}
