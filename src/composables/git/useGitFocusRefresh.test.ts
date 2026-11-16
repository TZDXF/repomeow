import { describe, expect, it, vi } from "vitest";
import {
  createFocusRefreshGate,
  createGitFocusRefresh,
  createWindowFocusSignal,
  FOCUS_REFRESH_INTERVAL,
  notifyWindowFocus,
  TRAY_POPUP_LABEL,
  windowFocusVersion,
} from "./useGitFocusRefresh";
import { createGitCheckCoordinator, gitCheckCoordinator } from "@/lib/git-check-coordinator";

function freshCoordinator(now: () => number) {
  return createGitCheckCoordinator({ now });
}

describe("createFocusRefreshGate", () => {
  it("首次放行,间隔内丢弃,到达间隔再次放行", () => {
    const gate = createFocusRefreshGate(FOCUS_REFRESH_INTERVAL);
    expect(gate.tryAcquire(1_000)).toBe(true);
    expect(gate.tryAcquire(1_000 + FOCUS_REFRESH_INTERVAL - 1)).toBe(false);
    expect(gate.tryAcquire(1_000 + FOCUS_REFRESH_INTERVAL)).toBe(true);
  });

  it("被丢弃的调用不刷新上次放行时间", () => {
    const gate = createFocusRefreshGate(10_000);
    expect(gate.tryAcquire(0)).toBe(true);
    for (let now = 1_000; now < 10_000; now += 500) {
      expect(gate.tryAcquire(now)).toBe(false);
    }
    expect(gate.tryAcquire(10_000)).toBe(true);
  });
});

describe("createGitFocusRefresh", () => {
  it("首次聚焦触发一次检查,间隔内的重复聚焦不再叠加", async () => {
    let now = 1_000;
    const refresh = vi.fn().mockResolvedValue(undefined),
      controller = createGitFocusRefresh(refresh, {
        target: () => "D:/repo",
        now: () => now,
        coordinator: freshCoordinator(() => now),
      });
    await controller.run();
    now += 10_000;
    await controller.run();
    expect(refresh).toHaveBeenCalledOnce();
    now += FOCUS_REFRESH_INTERVAL;
    await controller.run();
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("同一路径刚被其他入口检查过时直接复用,聚焦不再重扫(15s 保护)", async () => {
    const refresh = vi.fn().mockResolvedValue(undefined),
      coordinator = freshCoordinator(() => 1_000);
    coordinator.record("d:/repo", { modified: 0 });
    // 详情页刚完成的普通检查写入协调器后,紧接着的聚焦刷新必须复用该记录
    await createGitFocusRefresh(refresh, {
      target: () => String.raw`D:\Repo`,
      now: () => 1_500,
      coordinator,
    }).run();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("慢查询进行中时的再次聚焦复用同一请求,不叠加并发扫描", async () => {
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
        release = resolve;
      }),
      refresh = vi.fn(() => pending),
      controller = createGitFocusRefresh(refresh, { target: () => "D:/repo" }),
      first = controller.run(),
      second = controller.run();
    release();
    await Promise.all([first, second]);
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("页面未激活或没有目标路径时不触发检查", async () => {
    const refresh = vi.fn().mockResolvedValue(undefined);
    await createGitFocusRefresh(refresh, { target: () => null }).run();
    await createGitFocusRefresh(refresh, { target: () => "D:/repo", active: () => false }).run();
    expect(refresh).not.toHaveBeenCalled();
  });

  it("检查失败不写入有效期记录,间隔后的聚焦仍会重试", async () => {
    let now = 1_000;
    const refresh = vi
        .fn()
        .mockRejectedValueOnce(new Error("git unavailable"))
        .mockResolvedValue(undefined),
      controller = createGitFocusRefresh(refresh, {
        target: () => "D:/repo",
        now: () => now,
        coordinator: freshCoordinator(() => now),
      });
    await controller.run();
    now += FOCUS_REFRESH_INTERVAL;
    await controller.run();
    expect(refresh).toHaveBeenCalledTimes(2);
    // 第二次成功后写入记录,同一有效期内不再扫描
    await controller.run();
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("真实共享协调器:连续聚焦只扫描一次", async () => {
    gitCheckCoordinator.reset();
    const refresh = vi.fn().mockResolvedValue(undefined),
      controller = createGitFocusRefresh(refresh, { target: () => "D:/repo" });
    await controller.run();
    await controller.run();
    expect(refresh).toHaveBeenCalledOnce();
    gitCheckCoordinator.reset();
  });
});

describe("createWindowFocusSignal", () => {
  it("主窗口聚焦递增版本号,失焦不递增", async () => {
    let handler: ((focused: boolean) => void) | undefined;
    const before = windowFocusVersion.value,
      signal = createWindowFocusSignal({
        label: () => "main",
        subscribe: async (h) => {
          handler = h;
          return () => {
            handler = undefined;
          };
        },
      });
    await signal.start();
    handler!(false);
    expect(windowFocusVersion.value).toBe(before);
    handler!(true);
    expect(windowFocusVersion.value).toBe(before + 1);
    signal.stop();
    expect(handler).toBeUndefined();
  });

  it("托盘迷你弹窗不订阅窗口焦点", async () => {
    const subscribe = vi.fn(async () => () => {}),
      signal = createWindowFocusSignal({ label: () => TRAY_POPUP_LABEL, subscribe });
    await signal.start();
    expect(subscribe).not.toHaveBeenCalled();
  });

  it("start 期间已卸载时立即退订,不泄漏监听器", async () => {
    const unlisten = vi.fn();
    let resolveSubscribe!: (stop: () => void) => void;
    const signal = createWindowFocusSignal({
      label: () => "main",
      subscribe: () =>
        new Promise<() => void>((resolve) => {
          resolveSubscribe = () => resolve(unlisten);
        }),
    });
    const started = signal.start();
    signal.stop();
    resolveSubscribe(unlisten);
    await started;
    expect(unlisten).toHaveBeenCalledOnce();
  });
});

describe("notifyWindowFocus", () => {
  it("递增版本号供页面消费", () => {
    const before = windowFocusVersion.value;
    notifyWindowFocus();
    expect(windowFocusVersion.value).toBe(before + 1);
  });
});
