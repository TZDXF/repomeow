import { describe, expect, it } from "vitest";

import { parseWikiSources } from "@/lib/wiki-parse";

describe("parseWikiSources", () => {
  it("解析末尾 sources 注释块:剥离正文并提取行区间", () => {
    const content = `# 标题\n\n正文内容。\n\n<!-- sources\nsrc/lib/ai.ts:12-40\nsrc/lib/wiki.ts\n./src/views/ProjectWiki.vue:7\n-->\n`;
    const { body, ranges } = parseWikiSources(content);
    expect(body).toBe("# 标题\n\n正文内容。");
    expect(ranges.get("src/lib/ai.ts")).toEqual({ start: 12, end: 40 });
    // 只标单行时 end = start;纯路径条目不产生区间
    expect(ranges.get("src/views/ProjectWiki.vue")).toEqual({ start: 7, end: 7 });
    expect(ranges.has("src/lib/wiki.ts")).toBe(false);
  });

  it("无 sources 块时原样返回(仅去末尾空白)", () => {
    const { body, ranges } = parseWikiSources("# 标题\n\n正文。\n\n");
    expect(body).toBe("# 标题\n\n正文。");
    expect(ranges.size).toBe(0);
  });

  it("流式中途的未闭合 sources 尾巴被剥离,已闭合块不受影响", () => {
    const partial = "# 标题\n\n正文写到一半\n\n<!-- sources\nsrc/a.ts:1-";
    expect(parseWikiSources(partial).body).toBe("# 标题\n\n正文写到一半");
    const closed = "正文\n<!-- sources\nsrc/a.ts:1-9\n-->";
    const { body, ranges } = parseWikiSources(closed);
    expect(body).toBe("正文");
    expect(ranges.get("src/a.ts")).toEqual({ start: 1, end: 9 });
  });

  it("容忍非法条目:倒序区间 end 收敛为 start,起始行 <1 与非数字区间整条跳过", () => {
    const content = "正文\n<!-- sources\nsrc/a.ts:40-12\n:5-9\nsrc/b.ts:0-3\nsrc/c.ts:abc\n-->";
    const { ranges } = parseWikiSources(content);
    expect(ranges.get("src/a.ts")).toEqual({ start: 40, end: 40 });
    expect(ranges.has("")).toBe(false);
    expect(ranges.has("src/b.ts")).toBe(false);
    expect(ranges.get("src/c.ts")).toBeUndefined();
  });

  it("正文中的同名注释块不误剥:只处理最后一个块", () => {
    const content = "引用说明 <!-- sources -->\n\n正文\n\n<!-- sources\nsrc/a.ts:3-5\n-->";
    const { body, ranges } = parseWikiSources(content);
    expect(body).toContain("引用说明 <!-- sources -->");
    expect(ranges.get("src/a.ts")).toEqual({ start: 3, end: 5 });
  });

  it("knownFiles 提供时 bare filename 按 basename 补全为全路径", () => {
    const content = "正文\n<!-- sources\nai.ts:12-40\nsrc/wiki.ts:1-2\nunknown.ts:5\n-->";
    const known = ["src/lib/ai.ts", "src/wiki.ts"];
    const { ranges } = parseWikiSources(content, known);
    expect(ranges.get("src/lib/ai.ts")).toEqual({ start: 12, end: 40 });
    // 已是全路径的原样保留;查不到的全路径/bare name 保持原样(由调用方白名单过滤)
    expect(ranges.get("src/wiki.ts")).toEqual({ start: 1, end: 2 });
    expect(ranges.get("unknown.ts")).toEqual({ start: 5, end: 5 });
  });
});
