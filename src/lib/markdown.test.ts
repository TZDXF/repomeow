import { describe, expect, it } from "vitest";
import { safeLinkHref } from "./markdown";

describe("safeLinkHref", () => {
  it("放行 http/https/mailto 外链", () => {
    expect(safeLinkHref("https://example.com/docs")).toBe("https://example.com/docs");
    expect(safeLinkHref("http://example.com")).toBe("http://example.com");
    expect(safeLinkHref("mailto:a@b.com")).toBe("mailto:a@b.com");
    expect(safeLinkHref("HTTPS://EXAMPLE.COM")).toBe("HTTPS://EXAMPLE.COM");
  });

  it("放行页内锚点与相对/本地路径", () => {
    expect(safeLinkHref("#section-1")).toBe("#section-1");
    expect(safeLinkHref("./docs/guide.md")).toBe("./docs/guide.md");
    expect(safeLinkHref("../README.md")).toBe("../README.md");
    expect(safeLinkHref("/docs/guide.md")).toBe("/docs/guide.md");
    expect(safeLinkHref("assets/a.png?x=1#y")).toBe("assets/a.png?x=1#y");
  });

  it("Windows 盘符路径按本地路径放行(不误判为协议)", () => {
    expect(safeLinkHref("C:\\docs\\a.md")).toBe("C:\\docs\\a.md");
    expect(safeLinkHref("D:/code/proj/README.md")).toBe("D:/code/proj/README.md");
  });

  it("危险协议降级为 null", () => {
    expect(safeLinkHref("javascript:alert(1)")).toBeNull();
    expect(safeLinkHref("JAVASCRIPT:alert(1)")).toBeNull();
    expect(safeLinkHref("data:text/html,<script>alert(1)</script>")).toBeNull();
    expect(safeLinkHref("file:///etc/passwd")).toBeNull();
    expect(safeLinkHref("vbscript:msgbox(1)")).toBeNull();
  });

  it("协议中插入空白/控制符的绕过尝试同样拦截", () => {
    expect(safeLinkHref("java\tscript:alert(1)")).toBeNull();
    expect(safeLinkHref("  javascript:alert(1)  ")).toBeNull();
  });

  it("空值返回 null", () => {
    expect(safeLinkHref(null)).toBeNull();
    expect(safeLinkHref(undefined)).toBeNull();
    expect(safeLinkHref("")).toBeNull();
    expect(safeLinkHref("   ")).toBeNull();
  });
});
