import { describe, expect, it } from "vitest";
import { groupSemanticChanges, semanticTotal } from "@/lib/semantic";
import type { SemanticChange, SemanticDiffResult } from "@/types";

function change(path: string, name: string, line: number): SemanticChange {
  return {
    entityId: `${path}::function::${name}`,
    changeType: "modified",
    entityType: "function",
    entityName: name,
    startLine: line,
    endLine: line + 1,
    oldStartLine: null,
    oldEndLine: null,
    oldEntityName: null,
    filePath: path,
    oldFilePath: null,
    structuralChange: true,
  };
}

describe("groupSemanticChanges", () => {
  it("按文件与行号稳定分组", () => {
    const groups = groupSemanticChanges([
      change("src/z.ts", "z", 9),
      change("src/a.ts", "late", 20),
      { ...change("src/a.ts", "early", 2), changeType: "added" },
    ]);
    expect(groups.map((group) => group.path)).toEqual(["src/a.ts", "src/z.ts"]);
    expect(groups[0].changes.map((item) => item.entityName)).toEqual(["early", "late"]);
  });

  it("未知变化类型不会丢失", () => {
    const future = { ...change("a.ts", "x", 1), changeType: "future" };
    expect(groupSemanticChanges([future])[0].changes).toHaveLength(1);
  });
});

describe("semanticTotal", () => {
  it("空结果返回零", () => expect(semanticTotal(null)).toBe(0));

  it("读取汇总总数", () => {
    const result = {
      engineVersion: "0.23.1",
      summary: {
        fileCount: 1,
        added: 0,
        modified: 1,
        deleted: 0,
        moved: 0,
        renamed: 0,
        reordered: 0,
        binary: 0,
        orphan: 0,
        total: 7,
      },
      changes: [],
      binaryChanges: [],
    } satisfies SemanticDiffResult;
    expect(semanticTotal(result)).toBe(7);
  });
});
