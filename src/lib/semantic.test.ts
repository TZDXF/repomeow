import { describe, expect, it } from "vitest";
import {
  buildContextText,
  buildOutlineTree,
  compareEntitiesForDisplay,
  dedupeEntityRefs,
  entityDisplayName,
  flattenRelationGroups,
  sliceGraphSide,
  truncateGraphLabel,
} from "@/lib/semantic";
import type {
  SemanticContextResult,
  SemanticEntityRef,
  SemanticFileEntity,
  SemanticRelationGroup,
} from "@/types";

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

describe("sliceGraphSide", () => {
  it("caps shown nodes and reports the folded remainder", () => {
    const many = Array.from({ length: 10 }, (_, i) => ref({ name: `n${i}`, startLine: i }));
    const { shown, extra } = sliceGraphSide(many, 8);
    expect(shown).toHaveLength(8);
    expect(extra).toBe(2);
  });

  it("keeps all nodes without remainder when within the cap", () => {
    const few = [ref({ name: "a" }), ref({ name: "b" })];
    expect(sliceGraphSide(few)).toEqual({ shown: few, extra: 0 });
  });
});

describe("truncateGraphLabel", () => {
  it("truncates overlong labels with ellipsis", () => {
    expect(truncateGraphLabel("short")).toBe("short");
    const long = "a".repeat(30);
    const out = truncateGraphLabel(long);
    expect(out).toHaveLength(22);
    expect(out.endsWith("…")).toBe(true);
  });
});

// ── 第 4B 期:复制 AI 上下文 ──────────────────────────────────────────────────

function contextResult(partial: Partial<SemanticContextResult>): SemanticContextResult {
  return {
    engineVersion: "0.23.1",
    entity: "now_ts",
    entityId: "src-tauri/src/time_util.rs::function::now_ts",
    budget: 2000,
    totalTokens: 0,
    truncated: false,
    targetOmitted: false,
    entries: [],
    omitted: [],
    ...partial,
  };
}

describe("buildContextText", () => {
  it("returns empty string when there are no entries", () => {
    expect(buildContextText(contextResult({}))).toBe("");
  });

  it("renders one markdown section per entry with source content", () => {
    const text = buildContextText(
      contextResult({
        entries: [
          {
            entityId: "src/a.ts::function::foo",
            name: "foo",
            entityType: "function",
            filePath: "src/a.ts",
            role: "target",
            tokens: 12,
            content: "function foo() {\n  return 1;\n}\n",
          },
          {
            entityId: "src/b.ts::function::bar",
            name: "bar",
            entityType: "function",
            filePath: "src/b.ts",
            role: "direct_dependent",
            tokens: 8,
            content: "const bar = () => foo();",
          },
        ],
      }),
    );
    expect(text).toContain("# AI context: now_ts (src-tauri/src/time_util.rs::function::now_ts)");
    expect(text).toContain("## foo (function, src/a.ts, target, ~12 tokens)");
    expect(text).toContain("function foo() {");
    expect(text).toContain("## bar (function, src/b.ts, direct_dependent, ~8 tokens)");
    expect(text.endsWith("const bar = () => foo();")).toBe(true);
  });

  it("appends omitted groups and target-omitted note", () => {
    const text = buildContextText(
      contextResult({
        targetOmitted: true,
        entries: [
          {
            entityId: "src/a.ts::function::foo",
            name: "foo",
            entityType: "function",
            filePath: "src/a.ts",
            role: "target",
            tokens: 1,
            content: "foo",
          },
        ],
        omitted: [{ role: "direct_dependent", entities: 8, tests: 3 }],
      }),
    );
    expect(text).toContain("Omitted due to budget: 8 entities, 3 tests (direct_dependent)");
    expect(text).toContain("target entity source was omitted");
  });
});
