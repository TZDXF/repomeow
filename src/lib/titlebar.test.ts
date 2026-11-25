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

  it("交通灯占位足够容纳三个按钮且大于默认左内边距", () => {
    // 系统默认三个按钮在 x=9/32/55、宽 14,最右边缘 ≈ 69px
    expect(MACOS_TRAFFIC_LIGHT_INSET_PX).toBeGreaterThan(69);
    expect(MACOS_TRAFFIC_LIGHT_INSET_PX).toBeGreaterThan(TITLEBAR_EDGE_INSET_PX);
  });
});
