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
    const flow = useCommitAndPush(commit, push);
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
    const flow = useCommitAndPush(commit, push);
    await expect(flow.run()).rejects.toThrow("hook failed");
    expect(flow.pendingPush.value).toBe(false);
    expect(push).not.toHaveBeenCalled();
    await flow.run();
    expect(commit).toHaveBeenCalledTimes(2);
    expect(push).toHaveBeenCalledTimes(1);
  });
});
