import { describe, expect, it } from "vitest";
import { parseDiff, toSideBySideRows, type DiffFold } from "./diff";

describe("parseDiff", () => {
  it("忽略 hunk 之前的文件头(diff --git / index / --- /+++/空行),首个 hunk 之后的 hunk 头保留", () => {
    const text = [
      "diff --git a/a.txt b/a.txt",
      "index 1234..5678 100644",
      "--- a/a.txt",
      "+++ b/a.txt",
      "@@ -1,2 +1,2 @@",
      " line1",
      "-old",
      "+new",
    ].join("\n");
    const lines = parseDiff(text);
    expect(lines.map((l) => l.kind)).toEqual(["hunk", "ctx", "del", "add"]);
    // hunk 头是 add/del/ctx 的定位锚点,parseDiff 必须保留
    expect(lines[0].kind).toBe("hunk");
    expect(lines[0].text).toContain("@@ -1,2 +1,2 @@");
    // add / del 各自推进对应计数器
    expect(lines[2].oldLine).toBe(2);
    expect(lines[2].newLine).toBe(2);
    expect(lines[3].oldLine).toBeNull();
    expect(lines[3].newLine).toBe(2);
  });

  it("为 ctx / add / del 维护独立的 old/new 行号", () => {
    const text = ["@@ -10,4 +10,5 @@", " ctx1", " ctx2", "-old", "+new1", "+new2", " ctx3"].join(
      "\n",
    );
    const lines = parseDiff(text);
    expect(lines.map((l) => [l.kind, l.oldLine, l.newLine])).toEqual([
      ["hunk", null, null],
      ["ctx", 10, 10],
      ["ctx", 11, 11],
      ["del", 12, 12],
      ["add", null, 12],
      ["add", null, 13],
      ["ctx", 13, 14],
    ]);
  });

  it("首个 hunk 之前的 meta('Binary files ...')保留,普通文件头仍被忽略", () => {
    const text = [
      "diff --git a/img.png b/img.png",
      "Binary files a/img.png and b/img.png differ",
      "@@ -1 +0,0 @@",
      "-data",
    ].join("\n");
    const lines = parseDiff(text);
    expect(lines.map((l) => l.kind)).toEqual(["meta", "hunk", "del"]);
    expect(lines[0].text).toContain("Binary files");
  });

  it("\\ No newline at end of file 等杂项归为 meta,不参与行号", () => {
    const text = ["@@ -1 +1 @@", " old", "-old2", "\\ No newline at end of file", "+new"].join(
      "\n",
    );
    const lines = parseDiff(text);
    // 0 hunk, 1 " old"(ctx), 2 "-old2"(del), 3 "\\ No newline..."(meta), 4 "+new"(add)
    expect(lines.map((l) => l.kind)).toEqual(["hunk", "ctx", "del", "meta", "add"]);
  });
});

describe("toSideBySideRows", () => {
  it("ctx 行两侧同文", () => {
    const text = ["@@ -1 +1 @@", " same"].join("\n");
    const rows = toSideBySideRows(parseDiff(text));
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ kind: "hunk", text: expect.stringContaining("@@") });
    expect(rows[1].kind).toBe("line");
    expect(rows[1].left?.kind).toBe("ctx");
    expect(rows[1].right?.kind).toBe("ctx");
  });

  it("连续 del/add 按下标一一配对,多余一侧留空", () => {
    const text = ["@@ -1,3 +1,2 @@", "-only-del-1", "-only-del-2", "-shared", "+shared"].join("\n");
    const rows = toSideBySideRows(parseDiff(text));
    const lineRows = rows.filter((r) => r.kind === "line");
    // 真实实现按出现顺序 zip:第一个 add 配第一个 del,后续 del 全部留空
    // (unified diff 无替换信息,无法识别"中间 N 行被替换"的语义)
    expect(lineRows).toHaveLength(3);
    expect(lineRows[0].left?.kind).toBe("del");
    expect(lineRows[0].right?.kind).toBe("add");
    expect(lineRows[1].left?.kind).toBe("del");
    expect(lineRows[1].right).toBeNull();
    expect(lineRows[2].left?.kind).toBe("del");
    expect(lineRows[2].right).toBeNull();
  });

  it("空 diff / 空 hunk 边界:无 add/del 不产生 line 行", () => {
    const text = ["@@ -1 +1 @@", " only-ctx"].join("\n");
    const rows = toSideBySideRows(parseDiff(text));
    // hunk 1 行 + ctx 1 行
    expect(rows).toHaveLength(2);
    expect(rows.filter((r) => r.kind === "line")).toHaveLength(1);
  });

  it("全量 del(无 add)所有 line 行的 right 为 null", () => {
    const text = ["@@ -1,2 +0,0 @@", "-a", "-b"].join("\n");
    const rows = toSideBySideRows(parseDiff(text));
    const lineRows = rows.filter((r) => r.kind === "line");
    expect(lineRows).toHaveLength(2);
    expect(lineRows.every((r) => r.right === null)).toBe(true);
    expect(lineRows.every((r) => r.left?.kind === "del")).toBe(true);
  });

  it("全量 add(无 del)所有 line 行的 left 为 null", () => {
    const text = ["@@ -0,0 +1,2 @@", "+a", "+b"].join("\n");
    const rows = toSideBySideRows(parseDiff(text));
    const lineRows = rows.filter((r) => r.kind === "line");
    expect(lineRows).toHaveLength(2);
    expect(lineRows.every((r) => r.left === null)).toBe(true);
    expect(lineRows.every((r) => r.right?.kind === "add")).toBe(true);
  });

  it("fold 行整行通栏透传(不再展开为 line 行)", () => {
    const fold: DiffFold = { kind: "fold", count: 12, key: "0:12" };
    const text = ["@@ -1,2 +1,2 @@", " ctx", "+add"].join("\n");
    const parsed = parseDiff(text);
    const rows = toSideBySideRows([parsed[0], fold, ...parsed.slice(1)]);
    expect(rows[0].kind).toBe("hunk");
    expect(rows[1]).toMatchObject({ kind: "fold", fold });
    expect(rows[2].kind).toBe("line");
  });
});
