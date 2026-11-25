import { describe, expect, it } from "vitest";
import { isMacOS } from "./platform";

// 标题栏按平台分流的前提是平台判定可靠:WKWebView 的 UA 恒含 Macintosh,
// Windows WebView2 含 "Windows NT",Linux WebKitGTK 含 X11/Linux。
describe("isMacOS", () => {
  it("识别 macOS WKWebView", () => {
    expect(
      isMacOS(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
      ),
    ).toBe(true);
    // Apple Silicon 的 UA 同样报 Intel Mac OS X,不得按 CPU 判定
    expect(
      isMacOS("Mozilla/5.0 (Macintosh; Apple Silicon Mac OS X 14_0) AppleWebKit/605.1.15"),
    ).toBe(true);
  });

  it("不误判 Windows 与 Linux", () => {
    expect(
      isMacOS(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0",
      ),
    ).toBe(false);
    expect(
      isMacOS("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)"),
    ).toBe(false);
  });

  it("无 navigator 时回退为非 macOS,不抛错", () => {
    // 真的把全局 navigator 摘掉,走到 platform.ts 的 `typeof navigator === "undefined"` 分支;
    // 传空串只会命中正则负例,守不住这条回退(测完按原描述符还原,不影响后续用例)
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    delete (globalThis as { navigator?: unknown }).navigator;
    try {
      expect(isMacOS()).toBe(false);
    } finally {
      if (descriptor) {
        Object.defineProperty(globalThis, "navigator", descriptor);
      }
    }
    expect(typeof navigator).toBe("object");
  });

  it("有 navigator 时不传参默认读它的 UA", () => {
    // TitleBar.vue 就是无参调用 isMacOS();这条用例守住 platform.ts 的「读 navigator.userAgent」生产路径,
    // 缺失时把该行改成 `userAgent ?? ""` 全量仍是绿的(复验跑出过),mac 分支会静默失效。
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    Object.defineProperty(globalThis, "navigator", {
      value: {
        userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      },
      configurable: true,
    });
    try {
      expect(isMacOS()).toBe(true);
    } finally {
      if (descriptor) {
        Object.defineProperty(globalThis, "navigator", descriptor);
      } else {
        delete (globalThis as { navigator?: unknown }).navigator;
      }
    }
    // 与 :42 那条的 `typeof navigator === "object"` 对称:给「还原」本身配守门人。
    // 删掉上面整块 finally 时这行必红;没有它则没人能发现还原被删——乱序跑绿只能说明
    // 没有用例依赖残留状态,证明不了还原真的执行过。
    expect(navigator.userAgent).not.toContain("Macintosh");
  });
});
