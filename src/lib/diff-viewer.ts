import type { DiffFold, DiffLine, DiffSideRow } from "./diff";

export type DiffPaneSide = "left" | "right";

export interface DiffPaneRow {
  kind: "hunk" | "meta" | "line" | "fold";
  text: string;
  line: DiffLine | null;
  count: number;
  foldKey: string;
}

export interface DiffPaneRowOffsets {
  left: number[];
  right: number[];
}

export interface DiffChangeBlock {
  start: number;
  end: number;
  cls: "divider-del" | "divider-add" | "divider-mod";
  hasLeft: boolean;
  hasRight: boolean;
}

export interface DiffInsertMarker {
  top: number;
  cls: "insert-add" | "insert-del";
}

export interface DiffDividerShape {
  kind: "poly" | "line";
  d: string;
  y1: number;
  y2: number;
  cls: string;
}

/** 把较长的连续上下文压缩为两端上下文与一个可展开占位行。 */
export function foldContextLines(
  lines: DiffLine[],
  expanded: ReadonlySet<string>,
  options: { minRunLength?: number; edgeLength?: number } = {},
): (DiffLine | DiffFold)[] {
  const { minRunLength = 12, edgeLength = 3 } = options;
  const out: (DiffLine | DiffFold)[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind !== "ctx") {
      out.push(lines[i]);
      i++;
      continue;
    }
    let j = i;
    while (j < lines.length && lines[j].kind === "ctx") {
      j++;
    }
    const length = j - i;
    const key = `${i}:${length}`;
    if (length > minRunLength && !expanded.has(key)) {
      out.push(...lines.slice(i, i + edgeLength));
      out.push({ kind: "fold", count: length - edgeLength * 2, key });
      out.push(...lines.slice(j - edgeLength, j));
    } else {
      out.push(...lines.slice(i, j));
    }
    i = j;
  }
  return out;
}

/** 并排视图本侧没有内容时不插占位，保持 IntelliJ 默认的连续行布局。 */
export function buildPaneRows(rows: DiffSideRow[], side: DiffPaneSide): DiffPaneRow[] {
  const out: DiffPaneRow[] = [];
  for (const row of rows) {
    if (row.kind === "fold") {
      out.push({
        kind: "fold",
        text: "",
        line: null,
        count: row.fold?.count ?? 0,
        foldKey: row.fold?.key ?? "",
      });
      continue;
    }
    if (row.kind !== "line") {
      out.push({ kind: row.kind, text: row.text, line: null, count: 0, foldKey: "" });
      continue;
    }
    const line = side === "left" ? row.left : row.right;
    if (line) {
      out.push({ kind: "line", text: "", line, count: 0, foldKey: "" });
    }
  }
  return out;
}

/** 各规范 sideRow 在左右窗格中的起始行偏移，末尾元素是总行数哨兵。 */
export function buildPaneRowOffsets(rows: DiffSideRow[]): DiffPaneRowOffsets {
  const left: number[] = [];
  const right: number[] = [];
  let leftCount = 0;
  let rightCount = 0;
  for (const row of rows) {
    left.push(leftCount);
    right.push(rightCount);
    if (row.kind === "line") {
      if (row.left) {
        leftCount++;
      }
      if (row.right) {
        rightCount++;
      }
    } else {
      leftCount++;
      rightCount++;
    }
  }
  left.push(leftCount);
  right.push(rightCount);
  return { left, right };
}

/** 像素滚动位置转换成跨左右窗格共用的 sideRow 小数坐标。 */
export function locatePaneRowPosition(offsets: number[], scrollTop: number, rowHeight: number) {
  if (offsets.length < 2) {
    return 0;
  }
  const total = offsets[offsets.length - 1];
  const scaled = Math.min((scrollTop - rowHeight / 5) / rowHeight, total);
  if (scaled <= 0) {
    return scaled;
  }
  let low = 0;
  let high = offsets.length - 2;
  while (low < high) {
    const middle = (low + high + 1) >> 1;
    if (offsets[middle] <= scaled) {
      low = middle;
    } else {
      high = middle - 1;
    }
  }
  const segmentLength = offsets[low + 1] - offsets[low];
  return low + (segmentLength > 0 ? (scaled - offsets[low]) / segmentLength : 0);
}

/** sideRow 小数坐标转换为指定窗格的像素滚动位置。 */
export function paneScrollTopAt(offsets: number[], rowPosition: number, rowHeight: number) {
  if (rowPosition < 0) {
    return Math.max(rowHeight / 5 + rowPosition * rowHeight, 0);
  }
  if (offsets.length < 2) {
    return 0;
  }
  const index = Math.min(Math.max(Math.floor(rowPosition), 0), offsets.length - 2);
  const fraction = Math.min(Math.max(rowPosition - index, 0), 1);
  return (
    (offsets[index] + fraction * (offsets[index + 1] - offsets[index])) * rowHeight + rowHeight / 5
  );
}

/** 连续增删 sideRow 合并成连接条使用的变化块。 */
export function buildChangeBlocks(rows: DiffSideRow[]): DiffChangeBlock[] {
  const blocks: DiffChangeBlock[] = [];
  const isChange = (index: number) => {
    const row = rows[index];
    return row.kind === "line" && (row.left?.kind === "del" || row.right?.kind === "add");
  };
  let i = 0;
  while (i < rows.length) {
    if (!isChange(i)) {
      i++;
      continue;
    }
    let j = i;
    let hasDelete = false;
    let hasAdd = false;
    while (j < rows.length && isChange(j)) {
      if (rows[j].left?.kind === "del") {
        hasDelete = true;
      }
      if (rows[j].right?.kind === "add") {
        hasAdd = true;
      }
      j++;
    }
    let cls: DiffChangeBlock["cls"] = "divider-add";
    if (hasDelete && hasAdd) {
      cls = "divider-mod";
    } else if (hasDelete) {
      cls = "divider-del";
    }
    blocks.push({
      start: i,
      end: j,
      cls,
      hasLeft: hasDelete,
      hasRight: hasAdd,
    });
    i = j;
  }
  return blocks;
}

export function buildInsertMarkers(
  blocks: DiffChangeBlock[],
  offsets: DiffPaneRowOffsets,
  side: DiffPaneSide,
): DiffInsertMarker[] {
  const has = side === "left" ? "hasLeft" : "hasRight";
  return blocks
    .filter((block) => !block[has])
    .map((block) => ({
      top: offsets[side][block.start],
      cls: block.hasRight ? "insert-add" : "insert-del",
    }));
}

interface DividerShapeOptions {
  rows: DiffSideRow[];
  blocks: DiffChangeBlock[];
  offsets: DiffPaneRowOffsets;
  width: number;
  viewportHeight: number;
  leftScrollTop: number;
  rightScrollTop: number;
  rowHeight: number;
}

export function buildDividerShapes(options: DividerShapeOptions): DiffDividerShape[] {
  const { rows, blocks, offsets, width, viewportHeight, leftScrollTop, rightScrollTop, rowHeight } =
    options;
  const yLeft = (position: number) =>
    paneScrollTopAt(offsets.left, position, rowHeight) - leftScrollTop;
  const yRight = (position: number) =>
    paneScrollTopAt(offsets.right, position, rowHeight) - rightScrollTop;
  const shapes: DiffDividerShape[] = [];
  rows.forEach((row, index) => {
    if (row.kind !== "fold") {
      return;
    }
    const leftY = yLeft(index) + rowHeight / 2;
    const rightY = yRight(index) + rowHeight / 2;
    if (
      leftY >= -rowHeight &&
      rightY >= -rowHeight &&
      (leftY <= viewportHeight + rowHeight || rightY <= viewportHeight + rowHeight)
    ) {
      shapes.push({
        kind: "line",
        d: "",
        y1: leftY,
        y2: rightY,
        cls: "divider-fold",
      });
    }
  });
  for (const block of blocks) {
    const leftTop = yLeft(block.start);
    const leftBottom = yLeft(block.end);
    const rightTop = yRight(block.start);
    const rightBottom = yRight(block.end);
    if (
      (leftBottom >= -rowHeight || rightBottom >= -rowHeight) &&
      (leftTop <= viewportHeight + rowHeight || rightTop <= viewportHeight + rowHeight)
    ) {
      const middle = width / 2;
      shapes.push({
        kind: "poly",
        d: `M0,${leftTop} C${middle},${leftTop} ${middle},${rightTop} ${width},${rightTop} L${width},${rightBottom} C${middle},${rightBottom} ${middle},${leftBottom} 0,${leftBottom} Z`,
        y1: 0,
        y2: 0,
        cls: block.cls,
      });
    }
  }
  return shapes;
}

/** 取布尔序列中每个 false→true 连续块的首下标。 */
export function blockStarts(flags: boolean[]) {
  const starts: number[] = [];
  flags.forEach((flag, index) => {
    if (flag && !flags[index - 1]) {
      starts.push(index);
    }
  });
  return starts;
}
