/** diff 逐行解析结果(提交详情面板 / 提交对话框变更预览共用) */
export interface DiffLine {
  kind: "hunk" | "add" | "del" | "ctx" | "meta";
  text: string;
  oldLine: number | null;
  newLine: number | null;
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
