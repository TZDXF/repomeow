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

// ── 第 2 期:语义导航 ─────────────────────────────────────────────────────────

import {
  buildOutlineTree,
  compareEntitiesForDisplay,
  dedupeEntityRefs,
  entityDisplayName,
  flattenRelationGroups,
} from "@/lib/semantic";
import type { SemanticEntityRef, SemanticFileEntity, SemanticRelationGroup } from "@/types";

function entity(partial: Partial<SemanticFileEntity> & { name: string }): SemanticFileEntity {
  return {
    entityId: null,
    entityType: "function",
    filePath: "src/a.ts",
    startLine: 1,
    endLine: 2,
    parentId: null,
    ...partial,
  };
}

function ref(partial: Partial<SemanticEntityRef> & { name: string }): SemanticEntityRef {
  return {
    entityId: null,
    entityType: "function",
    filePath: "src/a.ts",
    startLine: 1,
    endLine: 2,
    ...partial,
  };
}

describe("buildOutlineTree", () => {
  it("builds parent-child hierarchy and sorts siblings by line", () => {
    const tree = buildOutlineTree([
      entity({
        name: "inner",
        entityId: "f::function::outer::inner",
        parentId: "f::function::outer",
        startLine: 5,
      }),
      entity({ name: "b", entityId: "f::function::b", startLine: 20 }),
      entity({ name: "outer", entityId: "f::function::outer", startLine: 3 }),
    ]);
    expect(tree.map((n) => n.entity.name)).toEqual(["outer", "b"]);
    expect(tree[0].children.map((n) => n.entity.name)).toEqual(["inner"]);
  });

  it("keeps entities with unknown parent as roots", () => {
    const tree = buildOutlineTree([
      entity({ name: "orphan", parentId: "f::function::ghost", startLine: 9 }),
      entity({ name: "top", startLine: 1 }),
    ]);
    expect(tree.map((n) => n.entity.name)).toEqual(["top", "orphan"]);
  });

  it("keeps entities without entityId", () => {
    const tree = buildOutlineTree([entity({ name: "plain" })]);
    expect(tree).toHaveLength(1);
    expect(tree[0].entity.name).toBe("plain");
  });

  it("keeps every entity when entityId is duplicated (e.g. TS overloads)", () => {
    const tree = buildOutlineTree([
      entity({ name: "host", entityId: "f::class::Host", startLine: 1 }),
      entity({
        name: "item",
        entityId: "f::class::Host::item",
        parentId: "f::class::Host",
        startLine: 2,
      }),
      entity({
        name: "item",
        entityId: "f::class::Host::item",
        parentId: "f::class::Host",
        startLine: 5,
      }),
    ]);
    expect(tree).toHaveLength(1);
    expect(tree[0].children.map((n) => n.entity.startLine)).toEqual([2, 5]);
  });

  it("breaks parent cycles caused by duplicated ids instead of recursing forever", () => {
    const tree = buildOutlineTree([
      entity({ name: "a", entityId: "f::function::x", parentId: "f::function::y", startLine: 1 }),
      entity({ name: "b", entityId: "f::function::y", parentId: "f::function::x", startLine: 2 }),
    ]);
    // 环被断开:两个实体都仍可达,且没有节点互为祖先
    const names: string[] = [];
    const walk = (nodes: ReturnType<typeof buildOutlineTree>) => {
      for (const n of nodes) {
        names.push(n.entity.name);
        walk(n.children);
      }
    };
    walk(tree);
    expect(names.sort()).toEqual(["a", "b"]);
  });
});

describe("compareEntitiesForDisplay", () => {
  it("orders structural types before high-density types", () => {
    const fn = ref({ name: "z", entityType: "function", startLine: 100 });
    const prop = ref({ name: "a", entityType: "property", startLine: 1 });
    expect(compareEntitiesForDisplay(fn, prop)).toBeLessThan(0);
    const earlier = ref({ name: "b", startLine: 1 });
    const later = ref({ name: "a", startLine: 50 });
    expect(compareEntitiesForDisplay(earlier, later)).toBeLessThan(0);
    expect(
      compareEntitiesForDisplay(
        ref({ name: "x", entityType: "unknown_kind" }),
        ref({ name: "x", entityType: "function" }),
      ),
    ).toBeGreaterThan(0);
  });
});

describe("dedupeEntityRefs", () => {
  it("dedupes by entityId, falling back to file+name+line", () => {
    const a = ref({ name: "x", entityId: "f::function::x" });
    const b = ref({ name: "x", entityId: "f::function::x", startLine: 9 });
    const c = ref({ name: "x", filePath: "g.ts", startLine: 1 });
    const d = ref({ name: "x", filePath: "g.ts", startLine: 1 });
    const e = ref({ name: "x", filePath: "g.ts", startLine: 2 });
    expect(dedupeEntityRefs([a, b, c, d, e])).toHaveLength(3);
  });
});

describe("flattenRelationGroups", () => {
  it("flattens and dedupes related entities across groups", () => {
    const groups: SemanticRelationGroup[] = [
      { entity: ref({ name: "a" }), related: [ref({ name: "s", entityId: "id1" })] },
      {
        entity: ref({ name: "a" }),
        related: [ref({ name: "s", entityId: "id1" }), ref({ name: "t", entityId: "id2" })],
      },
    ];
    expect(flattenRelationGroups(groups).map((r) => r.entityId)).toEqual(["id1", "id2"]);
  });
});

describe("entityDisplayName", () => {
  it("falls back to entity type when name is empty", () => {
    expect(entityDisplayName({ name: "", entityType: "section" })).toBe("section");
    expect(entityDisplayName({ name: "run", entityType: "function" })).toBe("run");
  });
});
