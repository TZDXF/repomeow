import { describe, expect, it } from "vitest";
import { buildFindRegExp, collectMatches } from "@/lib/text-search";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 覆盖查找正则构造与匹配收集的关键路径:三种模式的转义/语义、
// 空查询与非法正则回退、行号回填(跨多行)、零宽匹配防死循环。
// 注:构造出的正则带 g 标志,断言一律用 String.match(其内部会重置 lastIndex,
// 复用同一实例连续 test 会因 lastIndex 前进而得到错误结果)。

const TEXT = "const hello = 1;\n// hello world\nsay hello!\nHELLO?";

describe("buildFindRegExp", () => {
  it("字面查询转义特殊字符,默认大小写不敏感", () => {
    const re = buildFindRegExp({
      text: "a.b",
      caseSensitive: false,
      wholeWord: false,
      useRegex: false,
    })!;
    expect("a.b".match(re)).toBeTruthy();
    expect("axb".match(re)).toBeNull(); // "." 未被当作通配
    const ci = buildFindRegExp({
      text: "hello",
      caseSensitive: false,
      wholeWord: false,
      useRegex: false,
    })!;
    expect("HELLO".match(ci)).toBeTruthy();
  });

  it("大小写敏感区分 HELLO", () => {
    const re = buildFindRegExp({
      text: "hello",
      caseSensitive: true,
      wholeWord: false,
      useRegex: false,
    })!;
    expect("hello".match(re)).toBeTruthy();
    expect("HELLO".match(re)).toBeNull();
  });

  it("全字匹配不命中子串", () => {
    const re = buildFindRegExp({
      text: "hello",
      caseSensitive: true,
      wholeWord: true,
      useRegex: false,
    })!;
    expect("say hello!".match(re)).toBeTruthy();
    expect("helloworld".match(re)).toBeNull();
  });

  it("正则模式原样使用", () => {
    const re = buildFindRegExp({
      text: "h(el|a)lo",
      caseSensitive: true,
      wholeWord: false,
      useRegex: true,
    })!;
    expect("halo".match(re)).toBeTruthy();
    expect("hello".match(re)).toBeTruthy();
  });

  it("空查询与非法正则返回 null", () => {
    expect(
      buildFindRegExp({ text: "  ", caseSensitive: false, wholeWord: false, useRegex: false }),
    ).toBeNull();
    expect(
      buildFindRegExp({ text: "(", caseSensitive: false, wholeWord: false, useRegex: true }),
    ).toBeNull();
  });
});

describe("collectMatches", () => {
  it("收集全部匹配并回填 1-based 行号", () => {
    const re = buildFindRegExp({
      text: "hello",
      caseSensitive: false,
      wholeWord: false,
      useRegex: false,
    })!;
    const ranges = collectMatches(TEXT, re);
    expect(ranges.map((r) => r.line)).toEqual([1, 2, 3, 4]);
    expect(ranges.map((r) => TEXT.slice(r.from, r.to))).toEqual([
      "hello",
      "hello",
      "hello",
      "HELLO",
    ]);
  });

  it("大小写敏感时只收集精确匹配", () => {
    const re = buildFindRegExp({
      text: "hello",
      caseSensitive: true,
      wholeWord: false,
      useRegex: false,
    })!;
    expect(collectMatches(TEXT, re).map((r) => r.line)).toEqual([1, 2, 3]);
  });

  it("零宽正则不死循环,结果为空", () => {
    const re = buildFindRegExp({
      text: "x*",
      caseSensitive: false,
      wholeWord: false,
      useRegex: true,
    })!;
    expect(collectMatches("abc", re)).toEqual([]);
  });
});
