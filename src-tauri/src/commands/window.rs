//! 窗口控制命令:托盘迷你弹窗与主窗口的显示/隐藏。

use tauri::AppHandle;

use crate::error::AppResult;
use crate::tray;

/// 显示主窗口并聚焦;带 project_id 时前端跳转到该项目详情页。
#[tauri::command]
pub fn show_main_window(app: AppHandle, project_id: Option<i64>) -> AppResult<()> {
    tray::show_main_window(&app, project_id);
    Ok(())
}

/// 隐藏托盘迷你弹窗(如弹窗内按下 Esc)。
#[tauri::command]
pub fn hide_tray_popup(app: AppHandle) -> AppResult<()> {
    tray::hide_popup(&app);
    Ok(())
}

/// 切换当前窗口的 WebView DevTools(开发者模式 F12)。
/// 依赖 Cargo `devtools` feature:release 构建默认裁剪 DevTools API,无该 feature 时此方法不存在。
#[tauri::command]
pub fn toggle_devtools(window: tauri::WebviewWindow) -> AppResult<()> {
    if window.is_devtools_open() {
        window.close_devtools();
    } else {
        window.open_devtools();
    }
    Ok(())
}
