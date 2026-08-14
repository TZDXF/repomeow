// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 文本查找的共享规则:FindBar(文件内查找,跑在 CodeMirror 文档上)与全文搜索
// 结果的片段高亮/跳转定位共用同一套构造,保证两种入口匹配口径一致
// (大小写敏感/全字匹配/正则,与后端 search_project_text 的 SearchMatcher 对齐:
// 字面查询转义、正则原样、全字包 \b,大小写经 i 标志控制)。

export interface FindQuery {
  text: string;
  caseSensitive: boolean;
  wholeWord: boolean;
  useRegex: boolean;
}

export interface TextRange {
  from: number;
  to: number;
  /** 1-based 行号 */
  line: number;
}

/** 构造查找正则;空查询或非法正则返回 null(调用方各自决定呈现) */
export function buildFindRegExp(q: FindQuery): RegExp | null {
  if (!q.text.trim()) return null;
  let source = q.useRegex ? q.text : q.text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  if (q.wholeWord) source = `\\b(?:${source})\\b`;
  try {
    return new RegExp(source, q.caseSensitive ? "g" : "gi");
  } catch {
    return null;
  }
}

/** 收集全文匹配并回填行号;零宽匹配(如 a*)跳过并推进防死循环 */
export function collectMatches(text: string, re: RegExp): TextRange[] {
  const ranges: TextRange[] = [];
  re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m[0].length > 0) ranges.push({ from: m.index, to: m.index + m[0].length, line: 0 });
    re.lastIndex = m.index + Math.max(m[0].length, 1);
  }
  fillLines(text, ranges);
  return ranges;
}

/** 为偏移范围回填 1-based 行号(行首偏移数组上二分) */
function fillLines(text: string, ranges: TextRange[]): void {
  if (!ranges.length) return;
  const starts: number[] = [0];
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  for (const r of ranges) {
    let lo = 0;
    let hi = starts.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (starts[mid] <= r.from) lo = mid;
      else hi = mid - 1;
    }
    r.line = lo + 1;
  }
}
