import { createMarkmendParser } from "vue-stream-markdown";
import { describe, expect, it } from "vitest";
import { withVisibleMarkdownFrontmatter } from "./markdown-frontmatter";

const parserCases = (["", "\uFEFF"] as const).flatMap((bom) =>
  (["\n", "\r\n"] as const).flatMap((eol) =>
    (["---", "..."] as const).map((closing) => ({
      bom,
      eol,
      closing,
      label: `${bom ? "BOM" : "无 BOM"} + ${eol === "\n" ? "LF" : "CRLF"} + ${closing}`,
    })),
  ),
);

describe("withVisibleMarkdownFrontmatter", () => {
  it("将 YAML 头部转换为代码块并保留正文", () => {
    const source = ["---", "name: demo-skill", "description: 演示技能", "---", "", "# 正文"].join(
      "\n",
    );
    expect(withVisibleMarkdownFrontmatter(source)).toBe(
      ["```yaml", "name: demo-skill", "description: 演示技能", "```", "", "# 正文"].join("\n"),
    );
  });

  it("支持 BOM、CRLF 与 YAML 文档结束标记", () => {
    const source = "\uFEFF---\r\nname: demo-skill\r\n...\r\n正文\r\n";
    expect(withVisibleMarkdownFrontmatter(source)).toBe(
      "```yaml\nname: demo-skill\r\n```\n正文\r\n",
    );
  });

  it.each([
    ["普通 Markdown", "# 正文\n\n---\n"],
    ["未闭合的头部", "---\nname: demo-skill\n# 正文\n"],
    ["非头部的分隔线", "# 正文\n\n---\nname: value\n---\n"],
  ])("不改变%s", (_label, source) => {
    expect(withVisibleMarkdownFrontmatter(source)).toBe(source);
  });

  it.each(parserCases)("真实解析器保留 YAML 与正文：$label", async ({ bom, eol, closing }) => {
    const source = `${bom}---${eol}name: demo-skill${eol}description: 演示技能${eol}${closing}${eol}${eol}# 正文${eol}`,
      output = withVisibleMarkdownFrontmatter(source),
      { document } = await createMarkmendParser().parse(output, "static");
    expect(document.nodes).toEqual([
      [
        "pre",
        { language: "yaml" },
        ["code", { class: "language-yaml" }, "name: demo-skill\ndescription: 演示技能"],
      ],
      ["h1", {}, "正文"],
    ]);
  });
});
