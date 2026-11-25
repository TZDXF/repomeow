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
    expect(isMacOS("")).toBe(false);
  });
});
