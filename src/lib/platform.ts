/**
 * 运行平台判定(前端)。
 *
 * macOS 的窗口标题栏走系统原生交通灯(`src-tauri/tauri.macos.conf.json` 的
 * `titleBarStyle: Overlay` + `decorations: true`),前端标题栏需要据此让出左上角空间并
 * 隐藏 Windows 风格的自绘最小化/最大化/关闭按钮;其余平台保持自绘标题栏不变。
 * WKWebView 的 UA 恒含 `Macintosh`,与 Tauri/Rust 侧 `cfg(target_os = "macos")` 一一对应。
 */
export function isMacOS(userAgent?: string): boolean {
  const ua = userAgent ?? (typeof navigator === "undefined" ? "" : navigator.userAgent);
  return /(?<macToken>Macintosh|Mac OS X)/i.test(ua);
}
