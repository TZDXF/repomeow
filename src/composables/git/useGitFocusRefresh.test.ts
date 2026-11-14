import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createFocusRefreshGate,
  createGitFocusRefresh,
  FOCUS_REFRESH_INTERVAL,
} from "./useGitFocusRefresh";

const listeners: ((event: { payload: boolean }) => void)[] = [],
  unlisten = vi.fn();

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onFocusChanged: (handler: (event: { payload: boolean }) => void) => {
      listeners.push(handler);
      return Promise.resolve(unlisten);
    },
  }),
}));

afterEach(() => {
  listeners.length = 0;
  unlisten.mockClear();
});

describe("createFocusRefreshGate", () => {
  it("首次聚焦放行,间隔内重复聚焦被丢弃,超过间隔再次放行", () => {
    const gate = createFocusRefreshGate(FOCUS_REFRESH_INTERVAL);
    expect(gate.tryAcquire(1_000)).toBe(true);
    expect(gate.tryAcquire(1_000 + FOCUS_REFRESH_INTERVAL - 1)).toBe(false);
    expect(gate.tryAcquire(1_000 + FOCUS_REFRESH_INTERVAL)).toBe(true);
  });

  it("被丢弃的聚焦不刷新上次放行时间,连续抖动不会饿死后续刷新", () => {
    const gate = createFocusRefreshGate(10_000);
    expect(gate.tryAcquire(0)).toBe(true);
    for (let now = 1_000; now < 10_000; now += 500) {
      expect(gate.tryAcquire(now)).toBe(false);
    }
    expect(gate.tryAcquire(30_000)).toBe(true);
  });
});

describe("createGitFocusRefresh", () => {
  it("激活态下聚焦触发刷新,失焦不触发", async () => {
    const refresh = vi.fn().mockResolvedValue(undefined),
      controller = createGitFocusRefresh(refresh);
    await controller.start();
    listeners[0]!({ payload: true });
    await Promise.resolve();
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("页面未激活时(缓存的项目页在后台)不触发刷新", async () => {
    const refresh = vi.fn().mockResolvedValue(undefined),
      active = vi.fn(() => false),
      controller = createGitFocusRefresh(refresh, { active });
    await controller.start();
    listeners[0]!({ payload: true });
    await Promise.resolve();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("间隔内的第二次聚焦被节流,失败刷新静默吞掉", async () => {
    const refresh = vi.fn().mockRejectedValue(new Error("git unavailable")),
      controller = createGitFocusRefresh(refresh, { now: () => 5_000 });
    await controller.start();
    listeners[0]!({ payload: true });
    await Promise.resolve();
    listeners[0]!({ payload: true });
    await Promise.resolve();
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("卸载后退订监听器,不再响应后续聚焦", async () => {
    const refresh = vi.fn().mockResolvedValue(undefined),
      controller = createGitFocusRefresh(refresh);
    await controller.start();
    controller.stop();
    expect(unlisten).toHaveBeenCalledOnce();
    listeners[0]!({ payload: true });
    await Promise.resolve();
    expect(refresh).not.toHaveBeenCalled();
  });
});
