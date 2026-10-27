import { describe, expect, it } from "vitest";
import { resolvePath, safeLinkHref, safeMarkdownImage } from "./markdown";

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

describe("safeMarkdownImage", () => {
  const root = "C:/work/project";
  const base = "C:/work/project/docs";
  it("允许项目内图片并规范化路径", () => {
    expect(safeMarkdownImage("../images/a.png?raw=1#x", base, root)).toBe(
      "C:/work/project/images/a.png",
    );
    expect(safeMarkdownImage("C:/work/project/img.png", base, root)).toBe(
      "C:/work/project/img.png",
    );
    expect(safeMarkdownImage("https://example.com/a.png", "", "")).toBe(
      "https://example.com/a.png",
    );
    expect(safeMarkdownImage("../a.png", "/home/u/project/docs", "/home/u/project")).toBe(
      "/home/u/project/a.png",
    );
  });
  it("拒绝越界、协议和缺失根目录", () => {
    for (const url of [
      "../../secret.png",
      "%2e%2e/%2e%2e/secret",
      "C:/work/project/../secret.png",
      "C:/work/project-other/a.png",
      "\\\\server\\share\\a.png",
      "//server/share/a.png",
      "asset://localhost/C:/secret",
      "file:///secret",
      "data:image/png;base64,AA",
      "%5c%5cserver%5cshare",
      "a%00.png",
    ]) {
      expect(safeMarkdownImage(url, base, root), url).toBeNull();
    }
    expect(safeMarkdownImage("a.png", "", "")).toBeNull();
    expect(safeMarkdownImage("a.png", "C:/other", root)).toBeNull();
  });
});

describe("resolvePath", () => {
  it("保留 POSIX 绝对路径的根斜杠", () => {
    expect(resolvePath("/home/user/project", "docs/a.md")).toBe("/home/user/project/docs/a.md");
  });
});
