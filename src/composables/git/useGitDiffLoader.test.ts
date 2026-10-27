import { describe, expect, it } from "vitest";
import { useLatestRequest } from "../useLatestRequest";
import { useGitDiffLoader } from "./useGitDiffLoader";
import type { GitCommitFileDiff } from "@/types";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
const diff = (value: string) => ({ marker: value }) as unknown as GitCommitFileDiff;

describe("latest requests and git diff loading", () => {
  it("invalidates earlier tokens, including explicit invalidation", () => {
    const request = useLatestRequest();
    const first = request.begin();
    const second = request.begin();
    expect(first()).toBe(false);
    expect(second()).toBe(true);
    request.invalidate();
    expect(second()).toBe(false);
  });

  it("ignores stale success, failure and completion even for the same file", async () => {
    const loader = useGitDiffLoader({ keepPrevious: true });
    const a = deferred<GitCommitFileDiff>();
    const b = deferred<GitCommitFileDiff>();
    const first = loader.load(() => a.promise);
    const second = loader.load(() => b.promise);
    a.reject(new Error("old"));
    await first;
    expect(loader.loading.value).toBe(true);
    expect(loader.error.value).toBe("");
    b.resolve(diff("new"));
    await second;
    expect(loader.diff.value).toEqual(diff("new"));
    expect(loader.loading.value).toBe(false);
    const c = deferred<GitCommitFileDiff>();
    const third = loader.load(() => c.promise);
    expect(loader.loading.value).toBe(false);
    loader.reset();
    c.resolve(diff("late"));
    await third;
    expect(loader.diff.value).toBeNull();
  });

  it("does not let an older success overwrite a newer result", async () => {
    const loader = useGitDiffLoader({ keepPrevious: true });
    const old = deferred<GitCommitFileDiff>();
    const current = deferred<GitCommitFileDiff>();
    const first = loader.load(() => old.promise);
    const second = loader.load(() => current.promise);
    current.resolve(diff("current"));
    await second;
    old.resolve(diff("old"));
    await first;
    expect(loader.diff.value).toEqual(diff("current"));
  });

  it("clears previous content when requested and reports current errors", async () => {
    const loader = useGitDiffLoader({ keepPrevious: false });
    await loader.load(async () => diff("first"));
    const pending = deferred<GitCommitFileDiff>();
    const run = loader.load(() => pending.promise);
    expect(loader.diff.value).toBeNull();
    expect(loader.loading.value).toBe(true);
    pending.reject(new Error("failed"));
    await run;
    expect(loader.error.value).toBe("Error: failed");
    expect(loader.loading.value).toBe(false);
  });
});
