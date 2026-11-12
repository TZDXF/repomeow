import { effectScope, nextTick } from "vue";
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyCliRevisions, onCliDataChanged } from "./cli-sync";

afterEach(() => vi.useRealTimers());

describe("CLI cache invalidation", () => {
  it("coalesces domains, serializes overlapping refreshes and ignores unrelated changes", async () => {
    let finish!: () => void;
    const refresh = vi
        .fn()
        .mockImplementationOnce(
          () =>
            new Promise<void>((resolve) => {
              finish = resolve;
            }),
        )
        .mockResolvedValue(undefined),
      scope = effectScope();
    scope.run(() => onCliDataChanged(["test-project", "test-tag"], refresh));
    applyCliRevisions({ "test-other": "1" });
    await nextTick();
    expect(refresh).toHaveBeenCalledTimes(0);
    applyCliRevisions({ "test-project": "1", "test-tag": "1" });
    await nextTick();
    applyCliRevisions({ "test-project": "2" });
    await nextTick();
    expect(refresh).toHaveBeenCalledTimes(1);
    finish();
    await vi.waitFor(() => expect(refresh).toHaveBeenCalledTimes(2));
    applyCliRevisions({ "test-project": "2" });
    await nextTick();
    expect(refresh).toHaveBeenCalledTimes(2);
    scope.stop();
  });

  it("retries a failed read and stops after the owning view is disposed", async () => {
    vi.useFakeTimers();
    const refresh = vi.fn().mockRejectedValueOnce(new Error("busy")).mockResolvedValue(undefined),
      scope = effectScope();
    scope.run(() => onCliDataChanged(["test-retry"], refresh));
    applyCliRevisions({ "test-retry": "1" });
    await nextTick();
    await vi.advanceTimersByTimeAsync(1000);
    expect(refresh).toHaveBeenCalledTimes(2);
    scope.stop();
    applyCliRevisions({ "test-retry": "2" });
    await nextTick();
    expect(refresh).toHaveBeenCalledTimes(2);
  });
});
