//! 系统托盘:图标/菜单、迷你项目列表弹窗的创建与定位。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};
#[cfg(target_os = "windows")]
use tauri::window::{Effect, EffectsBuilder};
#[cfg(target_os = "macos")]
use tauri::window::{Effect, EffectState, EffectsBuilder};
use tauri_plugin_store::StoreExt;

use crate::APP_DATA_DIR_NAME;

/// 迷你项目列表弹窗的窗口 label
pub(crate) const TRAY_POPUP_LABEL: &str = "tray-popup";
/// 主窗口 label(tauri.conf.json 默认)
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

const POPUP_WIDTH: f64 = 360.0;
const POPUP_HEIGHT: f64 = 480.0;
/// 弹窗与托盘图标/屏幕边缘的间距(px)
const POPUP_MARGIN: f64 = 12.0;
/// 单击响应延迟:双击事件总是先于第二击的 DoubleClick 到达两次 Click,
/// 延迟等待以区分单击/双击,避免双击时弹窗闪现
const CLICK_DELAY: Duration = Duration::from_millis(300);

/// 单击代际计数:每次单击/双击 +1,延迟任务仅在自己仍是最后一代时才执行
static CLICK_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 最近一次双击的时间戳(ms,UNIX epoch):双击后系统还会补发一次 Click(Up),
/// 用它来丢弃这次尾随单击,避免双击时弹窗被重新打开
static LAST_DOUBLE_CLICK_MS: AtomicU64 = AtomicU64::new(0);
/// 双击后忽略尾随单击的时间窗口
const DOUBLE_CLICK_SUPPRESS: Duration = Duration::from_millis(500);

/// 弹窗最近一次显示完成的时间戳(ms,UNIX epoch)
static LAST_POPUP_SHOWN_MS: AtomicU64 = AtomicU64::new(0);
/// 显示后忽略失焦自动收起的宽限期:首次显示时,顶层窗口先经 set_focus 拿到键盘焦点,
/// 随后 WebView2 子窗口初始化完成 / 页面 autofocus 把焦点移进子 HWND,顶层窗口会收到
/// 一次 WM_KILLFOCUS(tao 转为 Focused(false)),若照常 hide 弹窗便一闪即收
const BLUR_SUPPRESS: Duration = Duration::from_millis(800);

/// 弹窗显示后的宽限期内,失焦事件视为显示过程的焦点迁移,不应触发自动收起。
pub(crate) fn should_ignore_popup_blur() -> bool {
    now_millis().saturating_sub(LAST_POPUP_SHOWN_MS.load(Ordering::SeqCst))
        < BLUR_SUPPRESS.as_millis() as u64
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 从 ~/.repomeow/settings.json 读取字符串设置项(与前端 tauri-plugin-store 同一文件)。
/// 读取失败返回 None,调用方自行回退默认值。
pub(crate) fn read_setting_string(app: &AppHandle, key: &str) -> Option<String> {
    let path = app
        .path()
        .home_dir()
        .ok()?
        .join(APP_DATA_DIR_NAME)
        .join("settings.json");
    let store = app.store(path).ok()?;
    store.get(key)?.as_str().map(str::to_owned)
}

/// 加载托盘 i18n 资源文件,按 settings.json 中的 language 字段选择 zh-CN/en-US。
/// 资源文件 `src-tauri/i18n/tray/<lang>.json` 由 tauri-build 打包进 resources。
/// 加载失败时按 language 走对应语言的硬编码兜底(避免 en-US 用户资源加载失败
/// 时回退到中文菜单造成界面语言不一致)
fn load_tray_texts(app: &App) -> (String, String) {
    let lang = read_setting_string(&app.handle(), "language").unwrap_or_default();
    // 兜底文案按界面语言分支(原行为是统一中文,en-US 用户资源加载失败会看到
    // 英文界面 + 中文托盘菜单,界面风格不一致)
    let fallback_zh = ("显示主窗口".to_string(), "退出".to_string());
    let fallback_en = ("Show main window".to_string(), "Quit".to_string());
    let fallback = if lang == "en-US" {
        &fallback_en
    } else {
        &fallback_zh
    };
    let file_name = if lang == "en-US" { "en-US.json" } else { "zh-CN.json" };
    let path = match app
        .path()
        .resolve(format!("i18n/tray/{file_name}"), tauri::path::BaseDirectory::Resource)
    {
        Ok(p) => p,
        Err(_) => return fallback.clone(),
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return fallback.clone(),
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return fallback.clone(),
    };
    let open = value
        .get("showMainWindow")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.0.clone());
    let quit = value
        .get("quit")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.1.clone());
    (open, quit)
}

/// 创建托盘图标:左键单击切换迷你弹窗,左键双击显示主窗口,右键打开菜单。
pub(crate) fn setup(app: &App) -> tauri::Result<()> {
    let (open_text, quit_text) = load_tray_texts(app);
    let open_item = MenuItem::with_id(app, "tray-open", open_text, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "tray-quit", quit_text, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("RepoMeow")
        // 左键留给单击/双击弹窗行为,菜单仅在右键弹出
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-open" => show_main_window(app, None),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    position,
                    rect,
                    ..
                } => {
                    // 锚点取托盘图标矩形(而非鼠标指针位置);个别平台/状态下 rect 可能为空,回退到点击点
                    // rect 在 tauri 内是 Position/Size 枚举(可能为逻辑坐标),统一转物理坐标后再用
                    let scale = app
                        .primary_monitor()
                        .ok()
                        .flatten()
                        .map(|m| m.scale_factor())
                        .unwrap_or(1.0);
                    let icon_pos: PhysicalPosition<f64> = rect.position.to_physical(scale);
                    let icon_size: tauri::PhysicalSize<f64> = rect.size.to_physical(scale);
                    let anchor = if icon_size.width > 0.0 && icon_size.height > 0.0 {
                        IconRect {
                            center_x: icon_pos.x + icon_size.width / 2.0,
                            top: icon_pos.y,
                            bottom: icon_pos.y + icon_size.height,
                        }
                    } else {
                        IconRect {
                            center_x: position.x,
                            top: position.y,
                            bottom: position.y,
                        }
                    };
                    // 双击尾巴上的 Click(Up):直接忽略,不再排入延迟任务
                    let since_double = now_millis().saturating_sub(LAST_DOUBLE_CLICK_MS.load(Ordering::SeqCst));
                    if since_double < DOUBLE_CLICK_SUPPRESS.as_millis() as u64 {
                        return;
                    }
                    // 延迟触发:若随后到来双击则该代作废,弹窗不会闪现
                    let generation = CLICK_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
                    let app = app.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(CLICK_DELAY);
                        if CLICK_GENERATION.load(Ordering::SeqCst) == generation {
                            toggle_popup(&app, anchor);
                        }
                    });
                }
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    // 作废待处理的单击任务并记录双击时间(抑制随后的尾随 Click),直接打开主窗口
                    CLICK_GENERATION.fetch_add(1, Ordering::SeqCst);
                    LAST_DOUBLE_CLICK_MS.store(now_millis(), Ordering::SeqCst);
                    show_main_window(app, None);
                }
                _ => {}
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 显示并聚焦主窗口;带 project_id 时通知前端跳转项目详情页。
pub(crate) fn show_main_window(app: &AppHandle, project_id: Option<i64>) {
    hide_popup(app);
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        if let Some(id) = project_id {
            let _ = window.emit("main://navigate", serde_json::json!({ "projectId": id }));
        }
    }
}

/// 隐藏迷你弹窗(窗口保留以便下次快速显示)。
pub(crate) fn hide_popup(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(TRAY_POPUP_LABEL) {
        let _ = window.hide();
    }
}

/// 托盘图标矩形(物理坐标),作为弹窗定位锚点
#[derive(Clone, Copy)]
struct IconRect {
    /// 图标水平中心
    center_x: f64,
    /// 图标上边缘
    top: f64,
    /// 图标下边缘
    bottom: f64,
}

fn toggle_popup(app: &AppHandle, anchor: IconRect) {
    if let Some(window) = app.get_webview_window(TRAY_POPUP_LABEL) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            return;
        }
    }
    show_popup(app, anchor);
}

/// 在托盘图标附近显示迷你弹窗(首次调用时懒创建窗口)。
fn show_popup(app: &AppHandle, anchor: IconRect) {
    let window = match app.get_webview_window(TRAY_POPUP_LABEL) {
        Some(window) => window,
        None => {
            // 透明窗口 + 系统级背景模糊(Windows Acrylic / macOS Vibrancy),
            // 让玻璃拟态皮肤下弹窗能透出桌面;其余皮肤根节点仍是不透明底色,外观不变
            let mut builder = WebviewWindowBuilder::new(
                app,
                TRAY_POPUP_LABEL,
                WebviewUrl::App("index.html#/tray".into()),
            )
            .title("RepoMeow")
            .inner_size(POPUP_WIDTH, POPUP_HEIGHT)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .decorations(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .visible(false)
            .transparent(true);
            #[cfg(target_os = "windows")]
            {
                builder = builder.effects(EffectsBuilder::new().effect(Effect::Acrylic).build());
            }
            #[cfg(target_os = "macos")]
            {
                builder = builder.effects(
                    EffectsBuilder::new()
                        .effect(Effect::Popover)
                        .state(EffectState::FollowsWindowActiveState)
                        .build(),
                );
            }
            let result = builder.build();
            match result {
                Ok(window) => window,
                Err(e) => {
                    eprintln!("failed to create tray popup window: {e}");
                    return;
                }
            }
        }
    };

    // 默认:水平居中于托盘图标,位于其上方
    let mut x = anchor.center_x - POPUP_WIDTH / 2.0;
    let mut y = anchor.top - POPUP_HEIGHT - POPUP_MARGIN;
    if let Ok(Some(monitor)) = app.monitor_from_point(anchor.center_x, anchor.top) {
        let work = monitor.work_area();
        let (wx, wy) = (work.position.x as f64, work.position.y as f64);
        let (ww, wh) = (work.size.width as f64, work.size.height as f64);
        // 托盘位于工作区下半部分(任务栏在底部)时弹窗贴图标上方,否则贴图标下方
        let icon_mid_y = (anchor.top + anchor.bottom) / 2.0;
        y = if icon_mid_y > wy + wh / 2.0 {
            anchor.top - POPUP_HEIGHT - POPUP_MARGIN
        } else {
            anchor.bottom + POPUP_MARGIN
        };
        x = x.clamp(wx + 8.0, wx + ww - POPUP_WIDTH - 8.0);
        y = y.clamp(wy + 8.0, wy + wh - POPUP_HEIGHT - 8.0);
    }
    let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
    let _ = window.show();
    let _ = window.set_focus();
    LAST_POPUP_SHOWN_MS.store(now_millis(), Ordering::SeqCst);
    // 通知弹窗刷新项目列表(主窗口的数据变更不会同步到弹窗的独立 Pinia 实例)
    let _ = window.emit("tray-popup://refresh", ());
}
