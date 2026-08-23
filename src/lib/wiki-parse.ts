/** 归一化 LLM 给的文件路径:去 ./ 与 / 前缀、反斜杠转正斜杠 */
function normalizeFilePath(p: string): string {
  return p.replace(/\\/g, "/").replace(/^\.?\//, "");
}

// ── 页面来源引用(path:start-end 行级标注) ────────────────────────────────
// 页面提示词要求 LLM 在正文末尾输出不可见的 HTML 注释块:
//   <!-- sources
//   src/foo.ts:12-40
//   src/bar.ts
//   -->
// 渲染前剥离该块并把行区间合并进来源文件 chips(点击滚动并高亮对应行)。

/** 来源文件上的 1-based 行区间(闭区间) */
export interface WikiSourceRange {
  start: number;
  end: number;
}

export interface ParsedWikiSources {
  /** 剥离 sources 注释块后的正文(末尾空白一并去掉) */
  body: string;
  /** 归一化路径 → 行区间;仅记录显式标注了区间的条目 */
  ranges: Map<string, WikiSourceRange>;
}

/** 已闭合的 sources 注释块(取最后一个,防止正文里出现多个时误剥前文) */
const SOURCES_BLOCK_RE = /<!--\s*sources\s*\n([\s\S]*?)-->/g;
/** 流式生成中途的未闭合尾巴:sources 起始注释一直到文末(后面还有 --> 说明已闭合,不算尾巴) */
const SOURCES_TAIL_RE = /\s*<!--\s*sources(?![\s\S]*-->)[\s\S]*$/;
/** 单行条目:路径 + 可选 `:start` / `:start-end`(路径本身不含冒号,惰性匹配兜底) */
const SOURCE_LINE_RE = /^(.+?)(?::(\d+)(?:\s*-\s*(\d+))?)?$/;

/**
 * 解析页面正文末尾的来源注释块:返回剥离后的正文与各文件的行区间。
 * 无块/块内无合法条目时 ranges 为空,body 为原文(仅去未闭合尾巴);
 * knownFiles(通常为大纲 relevantFiles)提供时,bare filename 按 basename 补全为全路径
 * (参照 deepwiki-open 的容错:LLM 常省略目录前缀)
 */
export function parseWikiSources(content: string, knownFiles?: string[]): ParsedWikiSources {
  const ranges = new Map<string, WikiSourceRange>();
  let body = content.replace(SOURCES_TAIL_RE, "");

  const matches = [...body.matchAll(SOURCES_BLOCK_RE)];
  // tsconfig lib 为 ES2020,无 Array.prototype.at
  const last = matches[matches.length - 1];
  if (last) {
    // bare filename → 全路径的 basename 查表(同名取先出现者)
    const byBasename = new Map<string, string>();
    for (const f of knownFiles ?? []) {
      const base = f.slice(f.lastIndexOf("/") + 1);
      if (!byBasename.has(base)) byBasename.set(base, f);
    }
    for (const rawLine of last[1].split("\n")) {
      const m = rawLine.trim().match(SOURCE_LINE_RE);
      if (!m) continue;
      let path = normalizeFilePath(m[1].trim());
      if (!path) continue;
      if (!path.includes("/")) path = byBasename.get(path) ?? path;
      const start = m[2] ? Number.parseInt(m[2], 10) : null;
      const end = m[3] ? Number.parseInt(m[3], 10) : start;
      if (start == null || end == null || start < 1) continue;
      ranges.set(path, { start, end: Math.max(start, end) });
    }
    // 只剥离最后一个块(正文里如有同名注释视为内容保留)
    const idx = last.index ?? 0;
    body = body.slice(0, idx) + body.slice(idx + last[0].length);
  }

  return { body: body.trimEnd(), ranges };
}
