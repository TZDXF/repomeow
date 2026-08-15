/** diff 逐行解析结果(提交详情面板 / 提交对话框变更预览共用) */
export interface DiffLine {
  kind: "hunk" | "add" | "del" | "ctx" | "meta";
  text: string;
  oldLine: number | null;
  newLine: number | null;
}

/** 未更改区间折叠占位行(展示层生成,parseDiff 不产生) */
export interface DiffFold {
  kind: "fold";
  /** 被折叠的行数 */
  count: number;
  /** 展开状态键(同一份 diff 内唯一) */
  key: string;
}

/** 并排视图的一行:左右各一格;ctx 两侧同文,hunk/meta/fold 整行通栏 */
export interface DiffSideRow {
  kind: "hunk" | "meta" | "line" | "fold";
  /** hunk/meta 的通栏文本(line/fold 行为空串) */
  text: string;
  /** 旧版本侧(del / ctx),无对应行时为 null */
  left: DiffLine | null;
  /** 新版本侧(add / ctx),无对应行时为 null */
  right: DiffLine | null;
  /** fold 行的折叠信息(其余行为 null) */
  fold: DiffFold | null;
}

/** 解析 unified diff:逐行旧/新行号;文件头(diff --git / index / --- / +++ 等)不展示 */
export function parseDiff(text: string): DiffLine[] {
  const out: DiffLine[] = [];
  let oldN = 0;
  let newN = 0;
  let seenHunk = false;
  for (const raw of text.split("\n")) {
    if (!raw) {
      continue;
    }
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(raw);
    if (hunk) {
      oldN = Number(hunk[1]);
      newN = Number(hunk[2]);
      seenHunk = true;
      out.push({ kind: "hunk", text: raw, oldLine: null, newLine: null });
      continue;
    }
    // 首个 hunk 之前是文件头(diff --git / index / --- / +++ 等),仅保留二进制提示
    if (!seenHunk) {
      if (raw.startsWith("Binary files")) {
        out.push({ kind: "meta", text: raw, oldLine: null, newLine: null });
      }
      continue;
    }
    if (raw.startsWith("+")) {
      out.push({ kind: "add", text: raw, oldLine: null, newLine: newN });
      newN++;
    } else if (raw.startsWith("-")) {
      out.push({ kind: "del", text: raw, oldLine: oldN, newLine: newN });
      oldN++;
    } else if (raw.startsWith(" ")) {
      out.push({ kind: "ctx", text: raw, oldLine: oldN, newLine: newN });
      oldN++;
      newN++;
    } else {
      // "\ No newline at end of file" 等
      out.push({ kind: "meta", text: raw, oldLine: null, newLine: null });
    }
  }
  return out;
}

/**
 * 成对增删行的行内差异区间(行内高亮用):连续 del/add 块按下标配对,
 * 取去掉行首标记后文本的公共前/后缀,区间 [start, end) 为两侧各自不同的片段;
 * 区间基于各自文本的 UTF-16 下标(与 shiki token 拼接、字符串切片口径一致)。
 * 配对后文本完全相同(区间为空)的行不产生条目。
 */
export function intralineRanges(lines: DiffLine[]): Map<DiffLine, [number, number]> {
  const out = new Map<DiffLine, [number, number]>();
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind !== "del" && lines[i].kind !== "add") {
      i++;
      continue;
    }
    const dels: DiffLine[] = [];
    const adds: DiffLine[] = [];
    while (i < lines.length && (lines[i].kind === "del" || lines[i].kind === "add")) {
      const change = lines[i];
      if (change.kind === "del") {
        dels.push(change);
      } else if (change.kind === "add") {
        adds.push(change);
      }
      i++;
    }
    const n = Math.min(dels.length, adds.length);
    for (let j = 0; j < n; j++) {
      const a = dels[j].text.slice(1);
      const b = adds[j].text.slice(1);
      const maxPre = Math.min(a.length, b.length);
      let pre = 0;
      while (pre < maxPre && a[pre] === b[pre]) pre++;
      let suf = 0;
      while (suf < maxPre - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++;
      if (pre < a.length - suf) out.set(dels[j], [pre, a.length - suf]);
      if (pre < b.length - suf) out.set(adds[j], [pre, b.length - suf]);
    }
  }
  return out;
}

/** 把逐行 diff 配成并排行:连续 del/add 块按下标一一配对,多余的一侧留空;fold 行整行通栏透传 */
export function toSideBySideRows(lines: (DiffLine | DiffFold)[]): DiffSideRow[] {
  const out: DiffSideRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.kind === "hunk" || line.kind === "meta") {
      out.push({ kind: line.kind, text: line.text, left: null, right: null, fold: null });
      i++;
      continue;
    }
    if (line.kind === "fold") {
      out.push({ kind: "fold", text: "", left: null, right: null, fold: line });
      i++;
      continue;
    }
    if (line.kind === "ctx") {
      out.push({ kind: "line", text: "", left: line, right: line, fold: null });
      i++;
      continue;
    }
    const dels: DiffLine[] = [];
    const adds: DiffLine[] = [];
    while (i < lines.length && (lines[i].kind === "del" || lines[i].kind === "add")) {
      const change = lines[i];
      if (change.kind === "del") {
        dels.push(change);
      } else if (change.kind === "add") {
        adds.push(change);
      }
      i++;
    }
    const n = Math.max(dels.length, adds.length);
    for (let j = 0; j < n; j++) {
      out.push({
        kind: "line",
        text: "",
        left: dels[j] ?? null,
        right: adds[j] ?? null,
        fold: null,
      });
    }
  }
  return out;
}
