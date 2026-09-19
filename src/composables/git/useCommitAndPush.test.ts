import { ref } from "vue";
import { describe, expect, it, vi } from "vitest";
import { useCommitAndPush } from "./useCommitAndPush";

describe("useCommitAndPush", () => {
  it("提交成功后多次推送失败，重试只推送，不重复提交", async () => {
    const commit = vi.fn().mockResolvedValue(undefined);
    const push = vi
      .fn()
      .mockRejectedValueOnce(new Error("network"))
      .mockRejectedValueOnce(new Error("rejected"))
      .mockResolvedValue(undefined);
    const flow = useCommitAndPush(commit, push, () => ({ key: "repo/main" }));
    await expect(flow.run()).rejects.toThrow("network");
    expect(flow.pendingPush.value).toBe(true);
    await expect(flow.run()).rejects.toThrow("rejected");
    expect(flow.pendingPush.value).toBe(true);
    await flow.run();
    expect(commit).toHaveBeenCalledTimes(1);
    expect(push).toHaveBeenCalledTimes(3);
    expect(flow.pendingPush.value).toBe(false);
    await flow.run();
    expect(commit).toHaveBeenCalledTimes(2);
  });

  it("提交失败时不推送，保留提交阶段供重试", async () => {
    const commit = vi
      .fn()
      .mockRejectedValueOnce(new Error("hook failed"))
      .mockResolvedValue(undefined);
    const push = vi.fn().mockResolvedValue(undefined);
    const flow = useCommitAndPush(commit, push, () => ({ key: "repo/main" }));
    await expect(flow.run()).rejects.toThrow("hook failed");
    expect(flow.pendingPush.value).toBe(false);
    expect(push).not.toHaveBeenCalled();
    await flow.run();
    expect(commit).toHaveBeenCalledTimes(2);
    expect(push).toHaveBeenCalledTimes(1);
  });
});

describe("目标隔离", () => {
  it("工作树或分支切换不复用待推送状态，切回后仍可重试", async () => {
    const target = ref({ key: "a/main" });
    const commit = vi.fn().mockResolvedValue(undefined);
    const push = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
    const flow = useCommitAndPush(commit, push, () => ({ ...target.value }));
    await expect(flow.run()).rejects.toThrow("offline");
    for (const key of ["b/main", "a/feature"]) {
      target.value = { key };
      expect(flow.pendingPush.value).toBe(false);
      await flow.run();
      expect(commit).toHaveBeenLastCalledWith({ key });
    }
    target.value = { key: "a/main" };
    expect(flow.pendingPush.value).toBe(true);
    await flow.run();
    expect(commit).toHaveBeenCalledTimes(3);
    expect(push).toHaveBeenLastCalledWith({ key: "a/main" });
    expect(flow.pendingPush.value).toBe(false);
  });

  it("提交等待期间切换目标，仍推送原目标", async () => {
    const target = ref({ key: "a/main" });
    let finish!: () => void;
    const commit = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    const push = vi.fn().mockRejectedValue(new Error("offline"));
    const flow = useCommitAndPush(commit, push, () => ({ ...target.value }));
    const run = flow.run();
    target.value = { key: "b/main" };
    finish();
    await expect(run).rejects.toThrow("offline");
    expect(push).toHaveBeenCalledWith({ key: "a/main" });
    expect(flow.pendingPush.value).toBe(false);
    target.value = { key: "a/main" };
    expect(flow.pendingPush.value).toBe(true);
  });
});
