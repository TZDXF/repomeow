//! 系统托盘:图标/菜单、迷你项目列表弹窗的创建与定位。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[cfg(target_os = "macos")]
use tauri::window::{Effect, EffectState, EffectsBuilder};
#[cfg(target_os = "windows")]
use tauri::window::{Effect, EffectsBuilder};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_store::StoreExt;

/// 迷你项目列表弹窗的窗口 label
pub(crate) const TRAY_POPUP_LABEL: &str = "tray-popup";
/// 主窗口 label(tauri.conf.json 默认)
pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

const POPUP_WIDTH: f64 = 360.0;
const POPUP_HEIGHT: f64 = 480.0;
/// 弹窗与托盘图标/屏幕边缘的间距(px)
const POPUP_MARGIN: f64 = 12.0;
/// 单击响应延迟:双击事件总是先于第二击的 DoubleClick 到达两次 Click,
/// 延迟等待以区分单击/双击,避免双击时弹窗闪现。Windows 由系统派发 DoubleClick,
/// 固定 300ms 即可;macOS 没有 DoubleClick 事件,延迟取系统双击间隔(见 click_delay)
#[cfg(not(target_os = "macos"))]
const CLICK_DELAY: Duration = Duration::from_millis(300);
/// 读不到系统双击间隔时的兜底值(macOS 默认 0.5s)
#[cfg(target_os = "macos")]
const DOUBLE_CLICK_FALLBACK: Duration = Duration::from_millis(500);

/// 单击代际计数:每次单击/双击 +1,延迟任务仅在自己仍是最后一代时才执行
static CLICK_GENERATION: AtomicU64 = AtomicU64::new(0);
/// 最近一次双击的时间戳(ms,UNIX epoch):Windows 派发 DoubleClick 后系统还会补发
/// 一次 Click(Up),macOS 自行判定双击后同样会有尾随单击;两处都用它丢弃尾巴
static LAST_DOUBLE_CLICK_MS: AtomicU64 = AtomicU64::new(0);
/// 双击后忽略尾随单击的时间窗口
const DOUBLE_CLICK_SUPPRESS: Duration = Duration::from_millis(500);
/// 最近一次单击的时间戳(ms,UNIX epoch):macOS 托盘后端不派发 DoubleClick,
/// 只能靠两次 Click 的间隔判定双击;0 表示没有可配对的单击
#[cfg(target_os = "macos")]
static LAST_CLICK_MS: AtomicU64 = AtomicU64::new(0);

/// 上一次可配对的单击时间戳,0 表示没有。
/// 只有 macOS 需要自行配对(Windows 由系统派发 DoubleClick),其他平台恒为 0,
/// 使 Windows 的单击/双击分支与改动前完全一致。
fn pairable_click_ms() -> u64 {
    #[cfg(target_os = "macos")]
    {
        LAST_CLICK_MS.load(Ordering::SeqCst)
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

/// 记录/清除可配对的单击时间戳(macOS 专用状态,其他平台为空操作)。
fn set_pairable_click_ms(value: u64) {
    #[cfg(target_os = "macos")]
    {
        LAST_CLICK_MS.store(value, Ordering::SeqCst);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = value;
    }
}

/// 本次左键单击(Click Up)的归类。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeftClickKind {
    /// 普通单击:延迟后切换迷你弹窗
    Single,
    /// 双击:打开主窗口(仅 macOS 需要自己配对判定)
    Double,
    /// 双击后的尾随单击(Windows 的 Click(Up) 尾巴、macOS 双击后的补发):直接丢弃
    Trailing,
}

/// 归类一次左键 Click(Up)。
///
/// - 双击窗口内的尾随单击先于一切判定被丢弃,避免双击后弹窗又被重新打开;
/// - `last_click_ms` 为上一次可配对的单击时间戳(0 表示没有),macOS 用它把两次
///   Click(Up) 配成双击;Windows 由系统派发 DoubleClick,这里恒传 0,
///   因此 Windows 的分支行为保持原样(单击延迟切换、双击直接开主窗口)。
fn classify_left_click(
    now_ms: u64,
    last_click_ms: u64,
    last_double_click_ms: u64,
    double_click_window: Duration,
) -> LeftClickKind {
    if now_ms.saturating_sub(last_double_click_ms) < DOUBLE_CLICK_SUPPRESS.as_millis() as u64 {
        return LeftClickKind::Trailing;
    }
    if last_click_ms != 0
        && now_ms.saturating_sub(last_click_ms) <= double_click_window.as_millis() as u64
    {
        return LeftClickKind::Double;
    }
    LeftClickKind::Single
}

/// 单击延迟:必须不短于双击配对窗口,否则第一击的弹窗会在第二击判定双击前就闪出来。
fn click_delay() -> Duration {
    #[cfg(target_os = "macos")]
    {
        double_click_window()
    }
    #[cfg(not(target_os = "macos"))]
    {
        CLICK_DELAY
    }
}

/// 系统双击间隔(可被用户在「系统设置 → 鼠标/触控板」调整)。
#[cfg(target_os = "macos")]
fn double_click_window() -> Duration {
    let secs = objc2_app_kit::NSEvent::doubleClickInterval();
    if secs > 0.0 && secs.is_finite() {
        Duration::from_secs_f64(secs)
    } else {
        DOUBLE_CLICK_FALLBACK
    }
}

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
    crate::time_util::now_system()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 从 ~/.repomeow/settings.json 读取字符串设置项(与前端 tauri-plugin-store 同一文件)。
/// 读取失败返回 None,调用方自行回退默认值。
pub(crate) fn read_setting_string(app: &AppHandle, key: &str) -> Option<String> {
    let path = crate::app_data_dir(app).ok()?.join("settings.json");
    let store = app.store(path).ok()?;
    store.get(key)?.as_str().map(str::to_owned)
}

/// 加载托盘 i18n 资源文件,按 settings.json 中的 language 字段选择 zh-CN/en-US。
/// 资源文件 `src-tauri/i18n/tray/<lang>.json` 由 tauri-build 打包进 resources。
/// 加载失败时按 language 走对应语言的硬编码兜底(避免 en-US 用户资源加载失败
/// 时回退到中文菜单造成界面语言不一致)
fn load_tray_texts(app: &App) -> (String, String) {
    let lang = read_setting_string(app.handle(), "language").unwrap_or_default();
    // 兜底文案按界面语言分支(原行为是统一中文,en-US 用户资源加载失败会看到
    // 英文界面 + 中文托盘菜单,界面风格不一致)
    let fallback_zh = ("显示主窗口".to_string(), "退出".to_string());
    let fallback_en = ("Show main window".to_string(), "Quit".to_string());
    let fallback = if lang == "en-US" {
        &fallback_en
    } else {
        &fallback_zh
    };
    let file_name = if lang == "en-US" {
        "en-US.json"
    } else {
        "zh-CN.json"
    };
    let path = match app.path().resolve(
        format!("i18n/tray/{file_name}"),
        tauri::path::BaseDirectory::Resource,
    ) {
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
                    let now = now_millis();
                    let kind = classify_left_click(
                        now,
                        pairable_click_ms(),
                        LAST_DOUBLE_CLICK_MS.load(Ordering::SeqCst),
                        click_delay(),
                    );
                    match kind {
                        // 双击尾巴上的 Click(Up):直接忽略,不再排入延迟任务
                        LeftClickKind::Trailing => {}
                        LeftClickKind::Double => {
                            // 作废待处理的单击任务并记录双击时间(抑制随后的尾随 Click),
                            // 直接打开主窗口
                            set_pairable_click_ms(0);
                            CLICK_GENERATION.fetch_add(1, Ordering::SeqCst);
                            LAST_DOUBLE_CLICK_MS.store(now, Ordering::SeqCst);
                            show_main_window(app, None);
                        }
                        LeftClickKind::Single => {
                            set_pairable_click_ms(now);
                            // 延迟触发:若随后到来双击则该代作废,弹窗不会闪现
                            let generation = CLICK_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
                            let app = app.clone();
                            let delay = click_delay();
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                if CLICK_GENERATION.load(Ordering::SeqCst) == generation {
                                    // 弹窗已按这一击的结果处理,配对状态作废
                                    set_pairable_click_ms(0);
                                    toggle_popup(&app, anchor);
                                }
                            });
                        }
                    }
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
            .visible(false);
            // transparent 仅 Windows/Linux 的 WebviewWindowBuilder 提供,macOS 上无此方法
            #[cfg(not(target_os = "macos"))]
            {
                builder = builder.transparent(true);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 与 macOS 的双击配对窗口都按同一份 500ms 语义断言(避免用系统设置值,
    /// 该值随用户偏好变化)
    const PICK_WINDOW: Duration = Duration::from_millis(300);

    #[test]
    fn classify_left_click_marks_first_click_as_single() {
        assert_eq!(
            classify_left_click(10_000, 0, 0, PICK_WINDOW),
            LeftClickKind::Single
        );
    }

    #[test]
    fn classify_left_click_pairs_click_inside_window_as_double() {
        // 间隔 120ms < 300ms 的双击配对窗口
        assert_eq!(
            classify_left_click(10_120, 10_000, 0, PICK_WINDOW),
            LeftClickKind::Double
        );
        // 恰好等于窗口也按双击处理(系统把边界算在同一击)
        assert_eq!(
            classify_left_click(10_300, 10_000, 0, PICK_WINDOW),
            LeftClickKind::Double
        );
    }

    #[test]
    fn classify_left_click_treats_slow_second_click_as_single() {
        // 间隔 301ms > 窗口:两次独立单击,弹窗先收再开
        assert_eq!(
            classify_left_click(10_301, 10_000, 0, PICK_WINDOW),
            LeftClickKind::Single
        );
    }

    #[test]
    fn classify_left_click_drops_trailing_click_after_double() {
        // 双击后 500ms 内系统补发的 Click(Up) 一律丢弃,避免弹窗被重新打开
        assert_eq!(
            classify_left_click(10_100, 10_050, 10_000, PICK_WINDOW),
            LeftClickKind::Trailing
        );
        // 尾随单击优先于双击配对:即使上一次单击也在窗口内,仍按尾巴丢弃
        assert_eq!(
            classify_left_click(10_200, 10_150, 10_000, PICK_WINDOW),
            LeftClickKind::Trailing
        );
    }

    #[test]
    fn classify_left_click_accepts_click_after_double_window() {
        // 超过 500ms 抑制窗口的单击恢复正常,不因历史双击而永久吞掉点击
        assert_eq!(
            classify_left_click(10_600, 0, 10_000, PICK_WINDOW),
            LeftClickKind::Single
        );
    }

    /// Windows 由系统派发 DoubleClick,事件回调恒传 last_click_ms = 0:
    /// 无论间隔多短,都不会被判成 Double,分支行为与改动前一致
    #[test]
    fn classify_left_click_never_pairs_without_previous_click() {
        assert_eq!(
            classify_left_click(10_000, 0, 0, Duration::from_secs(10)),
            LeftClickKind::Single
        );
        assert_eq!(
            classify_left_click(10_001, 0, 0, Duration::from_secs(10)),
            LeftClickKind::Single
        );
    }

    /// macOS 的单击延迟取自系统双击间隔,必须是可用的正值(用户可调,但不会为 0/NaN)
    #[test]
    fn click_delay_is_usable() {
        let delay = click_delay();
        assert!(
            delay >= Duration::from_millis(50),
            "延迟过短会闪出弹窗: {delay:?}"
        );
        assert!(
            delay <= Duration::from_secs(3),
            "延迟过长会让单击明显迟钝: {delay:?}"
        );
    }

    #[test]
    fn trailing_suppression_window_matches_constant() {
        assert_eq!(DOUBLE_CLICK_SUPPRESS, Duration::from_millis(500));
        // 抑制窗口必须不短于双击配对窗口,否则双击尾巴会漏成一次单击
        assert!(DOUBLE_CLICK_SUPPRESS >= PICK_WINDOW);
    }
}
