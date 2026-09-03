import { describe, expect, it } from "vitest";
import { extractFileRefs, resolveRefPath } from "@/lib/ai-file-refs";

describe("extractFileRefs", () => {
  it("提取根目录与子目录的 @ 引用", () => {
    expect(extractFileRefs("详见 @AGENTS.md 与 @docs/guide.md 的说明")).toEqual([
      "AGENTS.md",
      "docs/guide.md",
    ]);
  });

  it("末段必须带扩展名,纯 @名字 不匹配", () => {
    expect(extractFileRefs("cc @user 与 @someone 讨论")).toEqual([]);
  });

  it("排除邮箱与 URL 路径段(@ 前是单词字符或 /)", () => {
    expect(extractFileRefs("mail a@b.com, https://x.com/@foo.md 不算")).toEqual([]);
  });

  it("同一文本可多次调用互不串状态(g 正则 lastIndex 复位)", () => {
    const text = "@a.md @b.md";
    expect(extractFileRefs(text)).toEqual(["a.md", "b.md"]);
    expect(extractFileRefs(text)).toEqual(["a.md", "b.md"]);
  });

  it("支持多级目录、点号与短横线段、多扩展名", () => {
    expect(extractFileRefs("@.cursor/rules/my-rule.mdc @dist/foo.tar.gz")).toEqual([
      ".cursor/rules/my-rule.mdc",
      "dist/foo.tar.gz",
    ]);
  });

  it("行尾句号不进引用", () => {
    expect(extractFileRefs("见 @README.md.")).toEqual(["README.md"]);
  });
});

describe("resolveRefPath", () => {
  it("根目录文件的引用相对项目根解析", () => {
    expect(resolveRefPath("CLAUDE.md", "docs/guide.md")).toBe("docs/guide.md");
  });

  it("子目录文件的引用相对文件所在目录解析", () => {
    expect(resolveRefPath(".cursor/rules/a.mdc", "shared/common.md")).toBe(
      ".cursor/rules/shared/common.md",
    );
  });

  it("归一化 ./ 与 ..", () => {
    expect(resolveRefPath(".claude/skills/foo/SKILL.md", "../../rules/base.md")).toBe(
      ".claude/rules/base.md",
    );
    expect(resolveRefPath("docs/a.md", "./b.md")).toBe("docs/b.md");
  });

  it("越出根目录的 .. 直接丢弃", () => {
    expect(resolveRefPath("CLAUDE.md", "../outside.md")).toBe("outside.md");
  });

  it("兼容反斜杠写法", () => {
    expect(resolveRefPath("docs/a.md", "sub\\b.md")).toBe("docs/sub/b.md");
  });
});
