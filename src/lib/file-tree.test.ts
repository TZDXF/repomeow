import { describe, expect, it } from "vitest";
import { buildFileTree, flatFileRows, flattenVisibleTree } from "@/lib/file-tree";

// ── 任务描述 ─────────────────────────────────────────────────────────────────
// 覆盖静态文件树的行化:flattenVisibleTree(折叠跳过子级/展开态/目录可展开/
// dim 与 title 谓词)与 flatFileRows(默认显示名/谓词覆盖/depth 0 平铺行)。

interface TestFile {
  path: string;
  status?: string;
}

function files(...paths: string[]): TestFile[] {
  return paths.map((path) => ({ path }));
}

describe("buildFileTree", () => {
  it("按目录聚合并排序(目录在前、同层名称序)", () => {
    const tree = buildFileTree(files("src/b.ts", "README.md", "src/a.ts"));
    expect(tree.map((n) => n.name)).toEqual(["src", "README.md"]);
    expect(tree[0].children.map((n) => n.name)).toEqual(["a.ts", "b.ts"]);
  });
});

describe("flattenVisibleTree", () => {
  const tree = buildFileTree(files("src/lib/a.ts", "src/b.ts", "README.md"));

  it("全展开时按 DFS 拍平,深度随层级递增", () => {
    const rows = flattenVisibleTree(tree, new Set());
    expect(rows.map((r) => [r.fullPath, r.depth])).toEqual([
      ["src", 0],
      ["src/lib", 1],
      ["src/lib/a.ts", 2],
      ["src/b.ts", 1],
      ["README.md", 0],
    ]);
  });

  it("折叠目录跳过其子级,目录行 expanded/expandable 如实标记", () => {
    const rows = flattenVisibleTree(tree, new Set(["src"]));
    expect(rows.map((r) => r.fullPath)).toEqual(["src", "README.md"]);
    expect(rows[0]).toMatchObject({ expanded: false, expandable: true, isDir: true });
    expect(rows[1]).toMatchObject({ expanded: false, expandable: false, isDir: false });
  });

  it("文件行 data 携带原条目,目录行为 null", () => {
    const rows = flattenVisibleTree(tree, new Set());
    expect(rows.find((r) => r.fullPath === "src")?.data).toBeNull();
    expect(rows.find((r) => r.fullPath === "README.md")?.data).toEqual({ path: "README.md" });
  });

  it("dim/title 谓词仅作用于文件行", () => {
    const marked = buildFileTree<TestFile>([
      { path: "src/a.ts", status: "M" },
      { path: "src/b.ts", status: "D" },
    ]);
    const rows = flattenVisibleTree(marked, new Set(), {
      dim: (f) => f.status === "D",
      title: (f) => `title:${f.path}`,
    });
    expect(rows.find((r) => r.fullPath === "src/a.ts")).toMatchObject({
      dimmed: false,
      title: "title:src/a.ts",
    });
    expect(rows.find((r) => r.fullPath === "src/b.ts")).toMatchObject({ dimmed: true });
    expect(rows.find((r) => r.fullPath === "src")?.title).toBeUndefined();
  });
});

describe("flatFileRows", () => {
  it("depth 0 平铺行,默认显示名为路径最后一段", () => {
    const rows = flatFileRows(files("src/a.ts", "b.md"));
    expect(rows.map((r) => [r.name, r.depth, r.isDir, r.expandable])).toEqual([
      ["a.ts", 0, false, false],
      ["b.md", 0, false, false],
    ]);
  });

  it("name/dim/title 谓词覆盖", () => {
    const rows = flatFileRows([{ path: "src/old.ts" }, { path: "src/new.ts" }], {
      name: (f) => (f.path.endsWith("new.ts") ? "old.ts → new.ts" : "old.ts"),
      dim: (f) => f.path.endsWith("new.ts"),
      title: (f) => `t:${f.path}`,
    });
    expect(rows[0]).toMatchObject({ name: "old.ts", dimmed: false, title: "t:src/old.ts" });
    expect(rows[1]).toMatchObject({ name: "old.ts → new.ts", dimmed: true });
  });
});
