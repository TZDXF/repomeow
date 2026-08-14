import { describe, expect, it } from "vitest";
import type { ProjectFileEntry } from "@/types";
import {
  buildVisibleRows,
  entryName,
  PREFETCH_MAX_PER_LEVEL,
  prefetchTargets,
  sortDirEntries,
} from "@/lib/lazy-file-tree";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 覆盖懒加载文件树纯逻辑:层内排序(目录在前/名称序)、预取目标(跳过排除目录与
// 上限截断)、可见行生成(折叠/展开下钻/加载占位/已知空目录标记/灰显透传)。

function entry(path: string, isDir = false, ignored = false): ProjectFileEntry {
  return { path, isDir, ignored };
}

describe("entryName", () => {
  it("取相对路径最后一段", () => {
    expect(entryName("src")).toBe("src");
    expect(entryName("src/lib/a.ts")).toBe("a.ts");
  });
});

describe("sortDirEntries", () => {
  it("目录在前、文件在后,同层按名称排序", () => {
    const sorted = sortDirEntries([
      entry("b.txt"),
      entry("src", true),
      entry("a.txt"),
      entry("docs", true),
    ]);
    expect(sorted.map((e) => e.path)).toEqual(["docs", "src", "a.txt", "b.txt"]);
  });

  it("不修改原数组", () => {
    const input = [entry("b.txt"), entry("a.txt")];
    sortDirEntries(input);
    expect(input.map((e) => e.path)).toEqual(["b.txt", "a.txt"]);
  });
});

describe("prefetchTargets", () => {
  it("只挑未排除的目录,文件与 ignored 目录跳过", () => {
    const targets = prefetchTargets([
      entry("src", true),
      entry("node_modules", true, true),
      entry("README.md"),
      entry("docs", true),
    ]);
    expect(targets).toEqual(["src", "docs"]);
  });

  it("超过上限截断", () => {
    const children = Array.from({ length: PREFETCH_MAX_PER_LEVEL + 20 }, (_, i) =>
      entry(`d${i}`, true),
    );
    expect(prefetchTargets(children)).toHaveLength(PREFETCH_MAX_PER_LEVEL);
  });
});

describe("buildVisibleRows", () => {
  // 树形:src/(已加载:a.ts、lib/未加载) docs/(未加载) README.md empty/(已加载,空)
  const childrenMap = new Map<string, ProjectFileEntry[]>([
    ["", [entry("src", true), entry("docs", true), entry("empty", true), entry("README.md")]],
    ["src", [entry("src/lib", true), entry("src/a.ts")]],
    ["empty", []],
  ]);

  it("全部折叠时只出根层", () => {
    const rows = buildVisibleRows(childrenMap, new Set());
    expect(rows.map((r) => r.fullPath)).toEqual(["src", "docs", "empty", "README.md"]);
    expect(rows.every((r) => r.depth === 0)).toBe(true);
  });

  it("展开已加载目录下钻,未加载目录追加加载占位行", () => {
    const rows = buildVisibleRows(childrenMap, new Set(["src", "docs"]));
    expect(rows.map((r) => [r.fullPath, r.depth, r.loading])).toEqual([
      ["src", 0, false],
      ["src/lib", 1, false],
      ["src/a.ts", 1, false],
      ["docs", 0, false],
      ["docs", 1, true], // 加载占位
      ["empty", 0, false],
      ["README.md", 0, false],
    ]);
    expect(rows.find((r) => r.loading)?.key).toBe("docs::__loading");
  });

  it("已知空目录标 emptyDir,展开也不产生子行", () => {
    const rows = buildVisibleRows(childrenMap, new Set(["empty"]));
    const empty = rows.find((r) => r.fullPath === "empty")!;
    expect(empty.emptyDir).toBe(true);
    expect(rows.filter((r) => r.depth > 0)).toHaveLength(0);
  });

  it("ignored 标记透传为 dimmed", () => {
    const map = new Map<string, ProjectFileEntry[]>([
      ["", [entry("logs", true, true), entry("src", true)]],
    ]);
    const rows = buildVisibleRows(map, new Set());
    expect(rows.find((r) => r.fullPath === "logs")?.dimmed).toBe(true);
    expect(rows.find((r) => r.fullPath === "src")?.dimmed).toBe(false);
  });

  it("根层未加载时无行", () => {
    expect(buildVisibleRows(new Map(), new Set())).toEqual([]);
  });
});
