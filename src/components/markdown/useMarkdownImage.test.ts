import { effectScope, ref } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cmd } from "@/lib/tauri";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useMarkdownImage } from "./useMarkdownImage";

vi.mock("@/lib/tauri", () => ({ cmd: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => `asset:${path}`),
}));

function deferred() {
  let resolve!: (value: string) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<string>((ok, fail) => {
    resolve = ok;
    reject = fail;
  });
  return { promise, resolve, reject };
}
async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}
beforeEach(() => vi.clearAllMocks());

describe("useMarkdownImage", () => {
  it("只展示后端解析的路径,并在图片或根目录变化时丢弃过期结果", async () => {
    const first = deferred(),
      second = deferred(),
      third = deferred();
    vi.mocked(cmd)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
      .mockReturnValueOnce(third.promise);
    const url = ref("a.png"),
      base = ref("C:/one/docs"),
      root = ref("C:/one");
    const scope = effectScope();
    const src = scope.run(() =>
      useMarkdownImage(
        () => url.value,
        () => base.value,
        () => root.value,
      ),
    )!;
    expect(cmd).toHaveBeenCalledWith("resolve_markdown_image", {
      root: "C:/one",
      imagePath: "C:/one/docs/a.png",
    });
    url.value = "b.png";
    second.resolve("C:/one/real-b.png");
    await settle();
    expect(src.value).toBe("asset:C:/one/real-b.png");
    first.resolve("C:/one/old.png");
    await settle();
    expect(src.value).toBe("asset:C:/one/real-b.png");
    root.value = "C:/two";
    expect(src.value).toBeUndefined();
    base.value = "C:/two/docs";
    third.resolve("C:/two/real.png");
    await settle();
    expect(src.value).toBe("asset:C:/two/real.png");
    expect(convertFileSrc).not.toHaveBeenCalledWith("C:/one/old.png");
    scope.stop();
  });
  it("越界、后端拒绝、组件卸载和远程图片的处理", async () => {
    const pending = deferred();
    vi.mocked(cmd).mockReturnValueOnce(pending.promise).mockRejectedValueOnce(new Error("invalid"));
    const url = ref("a.png");
    const scope = effectScope();
    const src = scope.run(() =>
      useMarkdownImage(
        () => url.value,
        () => "C:/root",
        () => "C:/root",
      ),
    )!;
    url.value = "../outside.png";
    pending.resolve("C:/root/a.png");
    await settle();
    expect(src.value).toBeUndefined();
    url.value = "b.png";
    await settle();
    expect(src.value).toBeUndefined();
    url.value = "https://example.com/img.png";
    expect(src.value).toBe("https://example.com/img.png");
    const last = deferred();
    vi.mocked(cmd).mockReturnValueOnce(last.promise);
    url.value = "c.png";
    scope.stop();
    last.resolve("C:/root/c.png");
    await settle();
    expect(src.value).toBeUndefined();
  });
});
