import { describe, expect, it } from "vitest";

import { parseWikiOutline } from "@/lib/wiki-parse";

const VALID_XML = `<wiki_structure>
  <title>RepoMeow Wiki</title>
  <description>A local project manager.</description>
  <sections>
    <section id="section-1">
      <title>Overview</title>
      <pages>overview architecture</pages>
    </section>
  </sections>
  <pages>
    <page id="overview">
      <title>项目概览</title>
      <description>What this project is</description>
      <importance>high</importance>
      <relevant_files>
        <file_path>README.md</file_path>
        <file_path>./package.json</file_path>
      </relevant_files>
      <related_pages>
        <related>architecture</related>
      </related_pages>
    </page>
    <page id="architecture">
      <title>Architecture &amp; Layers</title>
      <description>How it is organized</description>
      <importance>medium</importance>
      <relevant_files>
        <file_path>src\\lib\\ai.ts</file_path>
        <file_path>not/exist.ts</file_path>
      </relevant_files>
    </page>
  </pages>
</wiki_structure>`;

describe("parseWikiOutline", () => {
  it("解析合法 XML:结构信息、页面字段、章节分组", () => {
    const outline = parseWikiOutline(VALID_XML);
    expect(outline.title).toBe("RepoMeow Wiki");
    expect(outline.description).toBe("A local project manager.");
    expect(outline.pages).toHaveLength(2);

    const [p1, p2] = outline.pages;
    expect(p1.id).toBe("overview");
    expect(p1.file).toBe("01-overview.md");
    expect(p1.title).toBe("项目概览");
    expect(p1.importance).toBe("high");
    expect(p1.section).toBe("Overview");
    expect(p1.relatedPages).toEqual(["architecture"]);
    // 路径归一化:去 ./ 前缀、反斜杠转正斜杠
    expect(p1.relevantFiles).toEqual(["README.md", "package.json"]);
    expect(p2.relevantFiles).toEqual(["src/lib/ai.ts", "not/exist.ts"]);
    // XML 实体反转义
    expect(p2.title).toBe("Architecture & Layers");
  });

  it("validFiles 过滤幻觉路径", () => {
    const outline = parseWikiOutline(VALID_XML, new Set(["README.md", "src/lib/ai.ts"]));
    expect(outline.pages[0].relevantFiles).toEqual(["README.md"]);
    expect(outline.pages[1].relevantFiles).toEqual(["src/lib/ai.ts"]);
  });

  it("容忍 markdown fence 与前导噪音", () => {
    const wrapped = `好的,以下是结构:\n\`\`\`xml\n${VALID_XML}\n\`\`\``;
    const outline = parseWikiOutline(wrapped);
    expect(outline.pages).toHaveLength(2);
  });

  it("输出被截断时补合成闭合标签抢救已完整页面", () => {
    const truncated = VALID_XML.slice(0, VALID_XML.indexOf("<file_path>not/exist.ts</file_path>"));
    const outline = parseWikiOutline(truncated);
    // 第一页完整保留;第二页抢救出已闭合的字段
    expect(outline.pages.length).toBeGreaterThanOrEqual(2);
    expect(outline.pages[0].id).toBe("overview");
    expect(outline.pages[1].id).toBe("architecture");
  });

  it("重复 id 加序号后缀去重,文件名按顺序编号", () => {
    const xml = `<wiki_structure><pages>
      <page id="core"><title>A</title></page>
      <page id="core"><title>B</title></page>
      <page id="core"><title>C</title></page>
    </pages></wiki_structure>`;
    const outline = parseWikiOutline(xml);
    expect(outline.pages.map((p) => p.id)).toEqual(["core", "core-2", "core-3"]);
    expect(outline.pages.map((p) => p.file)).toEqual([
      "01-core.md",
      "02-core-2.md",
      "03-core-3.md",
    ]);
  });

  it("非法 importance 归一为 medium,缺失章节为 null", () => {
    const xml = `<wiki_structure><pages>
      <page id="p"><title>T</title><importance>critical</importance></page>
    </pages></wiki_structure>`;
    const outline = parseWikiOutline(xml);
    expect(outline.pages[0].importance).toBe("medium");
    expect(outline.pages[0].section).toBeNull();
  });

  it("无 wiki_structure 或无页面时抛错(交给调用方重试)", () => {
    expect(() => parseWikiOutline("抱歉,我无法完成")).toThrow();
    expect(() => parseWikiOutline("<wiki_structure><title>x</title></wiki_structure>")).toThrow();
  });
});
