import { describe, expect, it } from "vitest";
import { parseDiff, toSideBySideRows, type DiffLine } from "./diff";
import {
  blockStarts,
  buildChangeBlocks,
  buildDividerShapes,
  buildInsertMarkers,
  buildPaneRowOffsets,
  buildPaneRows,
  foldContextLines,
  locatePaneRowPosition,
  paneScrollTopAt,
} from "./diff-viewer";

function contextLines(count: number): DiffLine[] {
  return Array.from({ length: count }, (_, index) => ({
    kind: "ctx",
    text: ` line-${index + 1}`,
    oldLine: index + 1,
    newLine: index + 1,
  }));
}

describe("foldContextLines", () => {
  it("长上下文只保留两端并生成稳定的折叠键", () => {
    const folded = foldContextLines(contextLines(13), new Set());
    expect(folded).toHaveLength(7);
    expect(folded[3]).toEqual({ kind: "fold", count: 7, key: "0:13" });
    expect(folded[0]).toMatchObject({ text: " line-1" });
    expect(folded[6]).toMatchObject({ text: " line-13" });
  });

  it("展开指定区间且不折叠恰好达到阈值的上下文", () => {
    expect(foldContextLines(contextLines(13), new Set(["0:13"]))).toHaveLength(13);
    expect(foldContextLines(contextLines(12), new Set())).toHaveLength(12);
  });

  it("非上下文行会切断折叠区间并参与后续键下标", () => {
    const change: DiffLine = { kind: "add", text: "+new", oldLine: null, newLine: 7 };
    const folded = foldContextLines([...contextLines(13), change, ...contextLines(13)], new Set());
    expect(folded.filter((line) => line.kind === "fold")).toEqual([
      { kind: "fold", count: 7, key: "0:13" },
      { kind: "fold", count: 7, key: "14:13" },
    ]);
  });
});

describe("并排窗格行模型", () => {
  const rows = toSideBySideRows(
    parseDiff(["@@ -1,3 +1,3 @@", " same", "-old-1", "-old-2", "+new", " tail"].join("\n")),
  ).filter((row) => row.kind !== "hunk");

  it("本侧缺行时不插占位，同时偏移仍保留规范 sideRow 坐标", () => {
    expect(buildPaneRows(rows, "left")).toHaveLength(4);
    expect(buildPaneRows(rows, "right")).toHaveLength(3);
    expect(buildPaneRowOffsets(rows)).toEqual({
      left: [0, 1, 2, 3, 4],
      right: [0, 1, 2, 2, 3],
    });
  });

  it("左右像素位置可以经 sideRow 坐标稳定互换", () => {
    const offsets = buildPaneRowOffsets(rows);
    const rowHeight = 20;
    const leftTop = paneScrollTopAt(offsets.left, 2.5, rowHeight);
    const position = locatePaneRowPosition(offsets.left, leftTop, rowHeight);
    expect(position).toBeCloseTo(2.5);
    expect(paneScrollTopAt(offsets.right, position, rowHeight)).toBe(44);
    expect(locatePaneRowPosition(offsets.left, 0, rowHeight)).toBeCloseTo(-0.2);
  });
});

describe("并排变化块与连接标记", () => {
  it("连续替换、纯删除分别形成变化块，并在空白侧生成删除标记", () => {
    const rows = toSideBySideRows(
      parseDiff(["@@ -1,3 +1,2 @@", "-old", "+new", " same", "-removed"].join("\n")),
    ).filter((row) => row.kind !== "hunk");
    const offsets = buildPaneRowOffsets(rows);
    const blocks = buildChangeBlocks(rows);
    expect(blocks).toEqual([
      { start: 0, end: 1, cls: "divider-mod", hasLeft: true, hasRight: true },
      { start: 2, end: 3, cls: "divider-del", hasLeft: true, hasRight: false },
    ]);
    expect(buildInsertMarkers(blocks, offsets, "left")).toEqual([]);
    expect(buildInsertMarkers(blocks, offsets, "right")).toEqual([{ top: 2, cls: "insert-del" }]);

    const shapes = buildDividerShapes({
      rows,
      blocks,
      offsets,
      width: 20,
      viewportHeight: 200,
      leftScrollTop: 0,
      rightScrollTop: 0,
      rowHeight: 20,
    });
    expect(shapes).toHaveLength(2);
    expect(shapes.map((shape) => shape.cls)).toEqual(["divider-mod", "divider-del"]);
    expect(shapes[0].d).toContain("C10,");
  });

  it("连续布尔变化只返回各块首下标", () => {
    expect(blockStarts([false, true, true, false, true, false])).toEqual([1, 4]);
  });
});
