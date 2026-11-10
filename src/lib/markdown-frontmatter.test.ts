import { createMarkmendParser } from "vue-stream-markdown";
import { describe, expect, it } from "vitest";
import { withVisibleMarkdownFrontmatter } from "./markdown-frontmatter";

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
      "\uFEFF```yaml\nname: demo-skill\r\n```\n正文\r\n",
    );
  });

  it.each([
    ["普通 Markdown", "# 正文\n\n---\n"],
    ["未闭合的头部", "---\nname: demo-skill\n# 正文\n"],
    ["非头部的分隔线", "# 正文\n\n---\nname: value\n---\n"],
  ])("不改变%s", (_label, source) => {
    expect(withVisibleMarkdownFrontmatter(source)).toBe(source);
  });

  it("转换后的 YAML 会进入实际渲染节点", async () => {
    const parser = createMarkmendParser(),
      source = "---\nname: demo-skill\ndescription: 演示技能\n---\n\n# 正文",
      { document } = await parser.parse(withVisibleMarkdownFrontmatter(source), "static"),
      rendered = JSON.stringify(document.nodes);
    expect(rendered).toContain("demo-skill");
    expect(rendered).toContain("演示技能");
    expect(rendered).toContain("正文");
  });
});
