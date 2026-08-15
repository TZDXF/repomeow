import { describe, expect, it } from "vitest";
import { diffLangOf, tokensToLineHtml } from "./diff-highlight";

describe("diffLangOf", () => {
  it("常见扩展名映射到 shiki canonical 语言 id", () => {
    expect(diffLangOf("src/lib/diff.ts")).toBe("typescript");
    expect(diffLangOf("src/App.vue")).toBe("vue");
    expect(diffLangOf("a/b/component.TSX")).toBe("tsx");
    expect(diffLangOf("scripts/ci.yml")).toBe("yaml");
    expect(diffLangOf("README.md")).toBe("markdown");
  });

  it("无扩展名的 Dockerfile / Makefile 按文件名识别", () => {
    expect(diffLangOf("docker/Dockerfile")).toBe("dockerfile");
    expect(diffLangOf("Dockerfile.dev")).toBe("dockerfile");
    expect(diffLangOf("Makefile")).toBe("makefile");
  });

  it("未收录扩展名 / 无扩展名 / 点开头隐藏文件落 text", () => {
    expect(diffLangOf("a/b.xyz123")).toBe("text");
    expect(diffLangOf("LICENSE")).toBe("text");
    expect(diffLangOf(".gitignore")).toBe("text");
  });
});

describe("tokensToLineHtml", () => {
  it("带色 token 输出双主题 CSS 变量 span,文本转义 HTML", () => {
    const html = tokensToLineHtml([
      { content: "const", htmlStyle: { "--shiki-light": "#CF222E", "--shiki-dark": "#FF7B72" } },
      { content: " a<b" },
    ]);
    expect(html).toBe(
      '<span style="--shiki-light:#CF222E;--shiki-dark:#FF7B72">const</span> a&lt;b',
    );
  });

  it("单侧重色时另一侧补 inherit;空 token 跳过", () => {
    const html = tokensToLineHtml([
      { content: "", htmlStyle: { "--shiki-light": "#000000" } },
      { content: "x", htmlStyle: { "--shiki-dark": "#FFFFFF" } },
    ]);
    expect(html).toBe('<span style="--shiki-light:inherit;--shiki-dark:#FFFFFF">x</span>');
  });

  it("undefined / 全无色 token 返回无色文本(调用方按 falsy 之外的非空串走 v-html 也安全)", () => {
    expect(tokensToLineHtml(undefined)).toBe("");
    expect(tokensToLineHtml([{ content: "a&b" }])).toBe("a&amp;b");
  });
});
