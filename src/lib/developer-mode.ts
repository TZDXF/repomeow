/**
 * 开发者模式:F12 切换 WebView DevTools + Ctrl+Shift+E 元素源码选取(element-source-dev)。
 *
 * dev 构建(import.meta.env.DEV)恒启用,行为与历史一致;release 构建由设置页
 * 「开发者模式」开关控制(见 GeneralSettings → DeveloperSettings)。
 * element-source-dev 经动态 import 加载,未开启时不会打进首屏 bundle(仅异步 chunk)。
 */
import { cmd } from "@/lib/tauri";
import type { ElementDevController } from "element-source-dev";

let controller: ElementDevController | null = null,
  /** 动态 import 的进行中 Promise,避免 enable/disable 快速连点时重复加载 */
  loading: Promise<ElementDevController> | null = null,
  enabled = false;

function onKeydown(e: KeyboardEvent) {
  if (e.key !== "F12") {
    return;
  }
  e.preventDefault();
  // fire-and-forget:开关 DevTools 无返回值,失败(webview 不支持)静默忽略
  void cmd("toggle_devtools").catch(() => {});
}

async function loadController(): Promise<ElementDevController> {
  if (controller) {
    return controller;
  }
  if (!loading) {
    loading = import("element-source-dev").then(({ default: createElementDev }) => {
      const c = createElementDev();
      controller = c;
      loading = null;
      return c;
    });
  }
  return loading;
}

/** 启用开发者模式:注册 F12 监听并加载元素选取工具(幂等) */
export async function enableDeveloperMode() {
  if (enabled) {
    return;
  }
  enabled = true;
  window.addEventListener("keydown", onKeydown);
  await loadController();
}

/** 关闭开发者模式:移除 F12 监听并销毁元素选取工具(幂等) */
export function disableDeveloperMode() {
  if (!enabled) {
    return;
  }
  enabled = false;
  window.removeEventListener("keydown", onKeydown);
  controller?.destroy();
  controller = null;
}
