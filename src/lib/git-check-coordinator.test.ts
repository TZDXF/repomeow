import { describe, expect, it, vi } from "vitest";
import { createGitCheckCoordinator, GIT_STATUS_TTL_MS, gitCheckKey } from "./git-check-coordinator";

describe("gitCheckKey", () => {
  it("归一化分隔符与大小写,同一仓库的不同写法共享记录", () => {
    expect(gitCheckKey(String.raw`D:\Code\Repo`)).toBe("d:/code/repo");
    expect(gitCheckKey("D:/Code/Repo/")).toBe("d:/code/repo");
    expect(gitCheckKey("  d:/code/repo  ")).toBe("d:/code/repo");
  });
});

describe("createGitCheckCoordinator", () => {
  it("有效期内的检查可复用,过期后视为需要重查", () => {
    let now = 1_000;
    const coordinator = createGitCheckCoordinator({ now: () => now });
    coordinator.record("D:/repo", { modified: 1 });
    expect(coordinator.fresh(String.raw`D:\repo`)).toBe(true);
    expect(coordinator.cached<{ modified: number }>("d:/repo")).toEqual({ modified: 1 });
    now += GIT_STATUS_TTL_MS - 1;
    expect(coordinator.fresh("D:/repo")).toBe(true);
    now += 1;
    expect(coordinator.fresh("D:/repo")).toBe(false);
    expect(coordinator.cached("D:/repo")).toBeUndefined();
  });

  it("成功的检查写入记录,失败不占用有效期", () => {
    const coordinator = createGitCheckCoordinator({ now: () => 1_000 }),
      pending = Promise.resolve("running"),
      finish = coordinator.begin("d:/repo", pending);
    expect(coordinator.pending("d:/repo")).toBe(pending);
    expect(coordinator.fresh("d:/repo")).toBe(false);
    finish({ ok: false });
    expect(coordinator.pending("d:/repo")).toBeUndefined();
    expect(coordinator.fresh("d:/repo")).toBe(false);

    const finishOk = coordinator.begin("d:/repo", Promise.resolve("ok"));
    finishOk({ ok: true, value: "ok" });
    expect(coordinator.fresh("d:/repo")).toBe(true);
    expect(coordinator.cached("d:/repo")).toBe("ok");
  });

  it("forget 同时清除有效期记录与进行中请求(强制重扫入口使用)", () => {
    const coordinator = createGitCheckCoordinator({ now: () => 1_000 }),
      pending = Promise.resolve("running");
    coordinator.record("D:/repo", "stale");
    coordinator.begin("d:/repo", pending);
    coordinator.forget("D:/repo");
    expect(coordinator.fresh("d:/repo")).toBe(false);
    expect(coordinator.pending("d:/repo")).toBeUndefined();
  });

  it("系统时钟回拨时不冻结记录,按过期处理", () => {
    let now = 10_000;
    const coordinator = createGitCheckCoordinator({ now: () => now });
    coordinator.record("D:/repo", "status");
    now = 9_000;
    expect(coordinator.fresh("D:/repo")).toBe(false);
  });

  it("reset 后旧请求的结束回调不复活已清空的记录(测试隔离)", () => {
    const coordinator = createGitCheckCoordinator({ now: () => 1_000 }),
      finish = coordinator.begin("d:/repo", Promise.resolve("value"));
    coordinator.reset();
    expect(coordinator.fresh("d:/repo")).toBe(false);
    expect(coordinator.pending("d:/repo")).toBeUndefined();
    finish({ ok: true, value: "value" });
    expect(coordinator.fresh("d:/repo")).toBe(false);
  });

  it("被后登记请求取代的旧完成回调不改动记录", () => {
    const coordinator = createGitCheckCoordinator({ now: () => 1_000 }),
      first = coordinator.begin("d:/repo", Promise.resolve(1)),
      second = coordinator.begin("d:/repo", Promise.resolve(2));
    first({ ok: true, value: 1 });
    expect(coordinator.pending("d:/repo")).toBeDefined();
    expect(coordinator.fresh("d:/repo")).toBe(false);
    second({ ok: true, value: 2 });
    expect(coordinator.pending("d:/repo")).toBeUndefined();
    expect(coordinator.cached("d:/repo")).toBe(2);
  });

  it("默认时钟按 Date.now 计算有效期", () => {
    const spy = vi.spyOn(Date, "now").mockReturnValue(1_000);
    try {
      const coordinator = createGitCheckCoordinator();
      coordinator.record("D:/repo", "status");
      expect(coordinator.fresh("D:/repo")).toBe(true);
      spy.mockReturnValue(1_000 + GIT_STATUS_TTL_MS - 1);
      expect(coordinator.fresh("D:/repo")).toBe(true);
      // 恰好到达有效期边界即视为过期,与后端 STATUS_TTL 的 `<` 比较一致
      spy.mockReturnValue(1_000 + GIT_STATUS_TTL_MS);
      expect(coordinator.fresh("D:/repo")).toBe(false);
    } finally {
      spy.mockRestore();
    }
  });
});
