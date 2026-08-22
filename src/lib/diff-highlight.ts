import { codeToTokens, type BundledLanguage } from "shiki";
import type { DiffLine } from "@/lib/diff";
import { baseName } from "@/lib/path";

/**
 * diff 代码着色:把 diff 行按「旧版本 = ctx+del / 新版本 = ctx+add」重组为两段完整源码,
 * 经 shiki codeToTokens 双主题(github-light / github-dark,与 CommandEditor 一致)分词后,
 * 再按行序一一映射回 DiffLine。重组与展示用的是同一批行,映射不依赖绝对行号,
 * 因此后端截断(truncated)或多 hunk 缺上下文时逐行对齐仍然成立,只是跨缺口的多行 token 可能着色偏差。
 */

/** 超过该字符数跳过着色(后端对 >10 万行的文件已截断,这里再兜一道避免 oniguruma 卡 UI) */
const MAX_CHARS = 500_000;

/** 扩展名 → shiki 语言 id(只收 canonical 名,未收录的落 text 不着色) */
const EXT_TO_LANG: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "jsx",
  vue: "vue",
  svelte: "svelte",
  html: "html",
  htm: "html",
  css: "css",
  scss: "scss",
  sass: "sass",
  less: "less",
  json: "json",
  jsonc: "jsonc",
  json5: "json5",
  md: "markdown",
  markdown: "markdown",
  py: "python",
  rs: "rust",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  rb: "ruby",
  swift: "swift",
  dart: "dart",
  lua: "lua",
  scala: "scala",
  groovy: "groovy",
  pl: "perl",
  r: "r",
  sh: "shellscript",
  bash: "shellscript",
  zsh: "shellscript",
  ps1: "powershell",
  bat: "bat",
  cmd: "bat",
  yml: "yaml",
  yaml: "yaml",
  toml: "toml",
  xml: "xml",
  svg: "xml",
  sql: "sql",
  ini: "ini",
  conf: "ini",
  properties: "properties",
  graphql: "graphql",
  gql: "graphql",
  prisma: "prisma",
  diff: "diff",
  mk: "makefile",
};

/** 由文件路径推断 shiki 语言;无扩展名的 Dockerfile / Makefile 按文件名识别 */
export function diffLangOf(filePath: string): string {
  const name = baseName(filePath);
  const lower = name.toLowerCase();
  if (lower === "dockerfile" || lower.startsWith("dockerfile.")) return "dockerfile";
  if (lower === "makefile") return "makefile";
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return "text";
  return EXT_TO_LANG[name.slice(dot + 1).toLowerCase()] ?? "text";
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

interface LineToken {
  content: string;
  htmlStyle?: Record<string, string>;
}

/**
 * 一段文本拼成内联 HTML:shiki 颜色双变量(--shiki-light / --shiki-dark)照原样保留;
 * emphasis 命中([start, end),基于整行去掉行首标记后的 UTF-16 下标)的片段
 * 额外套行内差异底色 span(del → diff-word-del / add → diff-word-add,配色在使用方组件)
 */
function segmentHtml(
  content: string,
  light: string | undefined,
  dark: string | undefined,
  emphasis: [number, number] | undefined,
  emphasisCls: string,
  base: number,
): string {
  const colorOf = (s: string) =>
    !light && !dark
      ? escapeHtml(s)
      : `<span style="--shiki-light:${light ?? "inherit"};--shiki-dark:${dark ?? "inherit"}">${escapeHtml(s)}</span>`;
  if (!emphasis) return colorOf(content);
  const cs = Math.max(emphasis[0] - base, 0);
  const ce = Math.min(emphasis[1] - base, content.length);
  if (cs >= ce) return colorOf(content);
  let html = "";
  if (cs > 0) html += colorOf(content.slice(0, cs));
  html += `<span class="${emphasisCls}">${colorOf(content.slice(cs, ce))}</span>`;
  if (ce < content.length) html += colorOf(content.slice(ce));
  return html;
}

/**
 * 一行 token 拼成内联 HTML:只取颜色双变量(--shiki-light / --shiki-dark),
 * 字号/斜体等字体样式不取,与 CommandEditor 的高亮观感保持一致;
 * 上色 CSS(按 .dark 切换变量)在使用方组件里,这里只输出变量。
 */
export function tokensToLineHtml(
  tokens: LineToken[] | undefined,
  emphasis?: [number, number],
  emphasisCls = "",
): string {
  if (!tokens) return "";
  let html = "";
  let base = 0;
  for (const token of tokens) {
    if (!token.content) continue;
    html += segmentHtml(
      token.content,
      token.htmlStyle?.["--shiki-light"],
      token.htmlStyle?.["--shiki-dark"],
      emphasis,
      emphasisCls,
      base,
    );
    base += token.content.length;
  }
  return html;
}

/** 未着色(纯文本回退)行的行内差异 HTML:转义后按区间三段拼接 */
export function emphasisTextHtml(
  text: string,
  emphasis: [number, number],
  emphasisCls: string,
): string {
  return segmentHtml(text, undefined, undefined, emphasis, emphasisCls, 0);
}

/** 行内差异底色 class(del / add 两侧配色不同) */
export function wordClsOf(line: DiffLine): string {
  return line.kind === "del" ? "diff-word-del" : "diff-word-add";
}

const THEMES = { light: "github-light", dark: "github-dark" } as const;

/**
 * 逐行着色整份 diff:返回 DiffLine → 行内 HTML(不含行首 +/-/空格 标记)。
 * emphasis 为行内差异区间(见 diff.ts 的 intralineRanges),命中的行在着色结果上叠加差异底色。
 * 返回 null 表示不着色(空 diff / 超大文件 / 分词失败),调用方回退纯文本渲染。
 */
export async function highlightDiffLines(
  lines: DiffLine[],
  filePath: string,
  emphasis?: Map<DiffLine, [number, number]>,
): Promise<Map<DiffLine, string> | null> {
  const oldSrc: string[] = [];
  const oldRefs: DiffLine[] = [];
  const newSrc: string[] = [];
  const newRefs: DiffLine[] = [];
  let total = 0;
  for (const line of lines) {
    if (line.kind !== "ctx" && line.kind !== "del" && line.kind !== "add") continue;
    // 去掉行首 diff 标记才是源码;ctx 行同时属于新旧两侧
    const src = line.text.slice(1);
    total += src.length;
    if (line.kind !== "add") {
      oldSrc.push(src);
      oldRefs.push(line);
    }
    if (line.kind !== "del") {
      newSrc.push(src);
      newRefs.push(line);
    }
  }
  if (!newRefs.length || total > MAX_CHARS) return null;

  const lang = diffLangOf(filePath) as BundledLanguage;
  const out = new Map<DiffLine, string>();
  const htmlOf = (ref: DiffLine, tokens: LineToken[] | undefined) =>
    tokensToLineHtml(tokens, emphasis?.get(ref), wordClsOf(ref));
  try {
    // 先旧后新:ctx 行两侧文本相同,新侧结果覆盖旧侧,视觉效果一致
    if (oldRefs.length) {
      const { tokens } = await codeToTokens(oldSrc.join("\n"), {
        lang,
        themes: THEMES,
        defaultColor: false,
      });
      oldRefs.forEach((ref, i) => out.set(ref, htmlOf(ref, tokens[i])));
    }
    const { tokens } = await codeToTokens(newSrc.join("\n"), {
      lang,
      themes: THEMES,
      defaultColor: false,
    });
    newRefs.forEach((ref, i) => out.set(ref, htmlOf(ref, tokens[i])));
  } catch {
    return null;
  }
  return out;
}
