import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  MACOS_TRAFFIC_LIGHT_INSET_PX,
  TITLEBAR_EDGE_INSET_PX,
  resolveTitleBarLayout,
} from "./titlebar";

// macOS 走系统原生装饰(tauri.macos.conf.json),Windows/Linux 保持自绘标题栏。
// 这两条路径互斥,任何一条回退都会在真机上直接可见:macOS 上重复的关闭按钮、
// 压住交通灯的 tab 条,或 Windows 上消失的窗口按钮。
describe("标题栏布局决策", () => {
  it("Windows/Linux 保持原有自绘标题栏", () => {
    expect(resolveTitleBarLayout({ macOS: false })).toEqual({
      showWindowControls: true,
      handleDragRegionDblClick: true,
      leadInsetPx: TITLEBAR_EDGE_INSET_PX,
    });
    // 非 macOS 仍用 Tailwind 的 pl-3,与改动前的视觉一致
    expect(TITLEBAR_EDGE_INSET_PX).toBe(12);
  });

  it("macOS 隐藏自绘窗口按钮,由系统交通灯接管", () => {
    const layout = resolveTitleBarLayout({ macOS: true });
    expect(layout.showWindowControls).toBe(false);
    expect(layout.leadInsetPx).toBe(MACOS_TRAFFIC_LIGHT_INSET_PX);
  });

  it("macOS 双击缩放交给 Tauri 内置 drag.js,前端不再重复调用", () => {
    expect(resolveTitleBarLayout({ macOS: true }).handleDragRegionDblClick).toBe(false);
  });

  it("交通灯占位盖过配置文件里三个按钮的实际右边缘", () => {
    // 阈值从 tauri.macos.conf.json 的 trafficLightPosition 推导,避免配置改了却还在守旧基线:
    // tao 的 inset_traffic_lights() 按「close 落在 x、间距与按钮尺寸取系统默认值」摆位,
    // 本机实测间距 23pt、按钮 14pt,故右边缘 = x + 2×23 + 14 = x + 60。
    // {x:16} → 76px,占位 80px 才留出 4px 余量;把常量降到 70~76 时这条必须失败。
    const config = JSON.parse(
      readFileSync(
        path.resolve(import.meta.dirname, "../../src-tauri/tauri.macos.conf.json"),
        "utf8",
      ),
    ) as {
      app: { windows: { trafficLightPosition?: { x: number } }[] };
    };
    const trafficLightX = config.app.windows[0].trafficLightPosition?.x;
    expect(trafficLightX).toBeTypeOf("number");
    const rightEdgePx = (trafficLightX ?? 0) + 60;
    expect(MACOS_TRAFFIC_LIGHT_INSET_PX).toBeGreaterThan(rightEdgePx);
    expect(MACOS_TRAFFIC_LIGHT_INSET_PX).toBeGreaterThan(TITLEBAR_EDGE_INSET_PX);
  });
});
