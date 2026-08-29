import { describe, expect, it, vi } from "vitest";
import { isSemanticCanceled, useSemanticRequest } from "./useSemanticRequest";

vi.mock("@/lib/tauri", () => ({
  cmd: vi.fn(() => Promise.resolve(true)),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("useSemanticRequest", () => {
  it("stores result and clears loading", async () => {
    const req = useSemanticRequest(async (_id: string, value: number) => value * 2);
    const done = req.run(21);
    expect(req.loading.value).toBe(true);
    expect(await done).toBe(42);
    expect(req.result.value).toBe(42);
    expect(req.loading.value).toBe(false);
    expect(req.error.value).toBe("");
  });

  it("ignores stale responses from superseded runs", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    let call = 0;
    const req = useSemanticRequest(async (_id: string) => {
      call += 1;
      return call === 1 ? first.promise : second.promise;
    });
    const run1 = req.run();
    const run2 = req.run();
    second.resolve("new");
    await run2;
    first.resolve("old");
    await run1;
    expect(req.result.value).toBe("new");
    expect(req.loading.value).toBe(false);
  });

  it("keeps error from latest run and swallows canceled errors", async () => {
    const req = useSemanticRequest(async (_id: string, fail: string) => {
      if (fail === "plain") throw new Error("boom");
      const error = new Error("canceled") as Error & { code?: string };
      error.code = "semantic_canceled";
      throw error;
    });
    expect(await req.run("plain")).toBeNull();
    expect(req.error.value).toBe("boom");
    expect(await req.run("canceled")).toBeNull();
    expect(req.error.value).toBe("");
  });

  it("reset clears state and invalidates in-flight run", async () => {
    const pending = deferred<string>();
    const req = useSemanticRequest((_id: string) => pending.promise);
    const run = req.run();
    req.reset();
    pending.resolve("late");
    expect(await run).toBeNull();
    expect(req.result.value).toBeNull();
    expect(req.loading.value).toBe(false);
  });

  it("isSemanticCanceled matches by code only", () => {
    expect(isSemanticCanceled({ code: "semantic_canceled" })).toBe(true);
    expect(isSemanticCanceled(new Error("x"))).toBe(false);
    expect(isSemanticCanceled(null)).toBe(false);
  });
});
