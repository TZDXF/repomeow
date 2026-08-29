import type { SemanticChange, SemanticDiffResult } from "@/types";

export interface SemanticChangeGroup {
  path: string;
  changes: SemanticChange[];
}

const CHANGE_ORDER: Record<string, number> = {
  modified: 0,
  added: 1,
  deleted: 2,
  renamed: 3,
  moved: 4,
  reordered: 5,
};

/** 按当前文件路径分组；组内先按行号、再按变化类型与实体名稳定排序。 */
export function groupSemanticChanges(changes: SemanticChange[]): SemanticChangeGroup[] {
  const groups = new Map<string, SemanticChange[]>();
  for (const change of changes) {
    const group = groups.get(change.filePath);
    if (group) {
      group.push(change);
    } else {
      groups.set(change.filePath, [change]);
    }
  }
  return [...groups.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([path, values]) => ({
      path,
      changes: values.sort(
        (a, b) =>
          a.startLine - b.startLine ||
          (CHANGE_ORDER[a.changeType] ?? 99) - (CHANGE_ORDER[b.changeType] ?? 99) ||
          a.entityName.localeCompare(b.entityName),
      ),
    }));
}

export function semanticTotal(result: SemanticDiffResult | null): number {
  return result?.summary.total ?? 0;
}

// ── 第 2 期:语义导航(结构树 / 搜索 / 关系)─────────────────────────────────

import type {
  SemanticEntityRef,
  SemanticFileEntitiesResult,
  SemanticFileEntity,
  SemanticRelationGroup,
} from "@/types";

export interface SemanticOutlineNode {
  entity: SemanticFileEntity;
  children: SemanticOutlineNode[];
}

/**
 * 按 parentId 把文件实体拍平列表组装成树;parentId 指向不存在的实体时
 * 作为根节点(不丢数据)。同级按 startLine 稳定排序。
 * sem 对同名嵌套实体(如 TS 函数重载)会产生重复 entityId:id 索引只保留首个,
 * 且对父链做环检测——否则同一节点被多方引用成环,递归渲染会无限嵌套。
 */
export function buildOutlineTree(entities: SemanticFileEntity[]): SemanticOutlineNode[] {
  const byId = new Map<string, SemanticOutlineNode>();
  const all = entities.map<SemanticOutlineNode>((entity) => ({ entity, children: [] }));
  for (const node of all) {
    const id = node.entity.entityId;
    if (id && !byId.has(id)) byId.set(id, node);
  }
  const parentOf = new Map<SemanticOutlineNode, SemanticOutlineNode>();
  for (const node of all) {
    const parentId = node.entity.parentId;
    const parent = parentId ? byId.get(parentId) : undefined;
    if (parent && parent !== node) parentOf.set(node, parent);
  }
  // 环检测:沿父链向上走,回到自身即脱钩升为根(重复 id 互相引用会成环)
  for (const node of all) {
    const seen = new Set<SemanticOutlineNode>();
    let cur: SemanticOutlineNode | undefined = parentOf.get(node);
    while (cur) {
      if (cur === node || seen.has(cur)) {
        parentOf.delete(node);
        break;
      }
      seen.add(cur);
      cur = parentOf.get(cur);
    }
  }
  const roots: SemanticOutlineNode[] = [];
  const orphans: SemanticOutlineNode[] = [];
  for (const node of all) {
    const parent = parentOf.get(node);
    if (parent) parent.children.push(node);
    else if (node.entity.entityId) roots.push(node);
    else orphans.push(node);
  }
  const byLine = (a: SemanticOutlineNode, b: SemanticOutlineNode) =>
    a.entity.startLine - b.entity.startLine || a.entity.name.localeCompare(b.entity.name);
  const sortRecursive = (list: SemanticOutlineNode[]) => {
    list.sort(byLine);
    for (const item of list) sortRecursive(item.children);
  };
  sortRecursive(roots);
  sortRecursive(orphans);
  return [...roots, ...orphans];
}

/** 实体类型展示序:函数/类/方法/接口等结构性类型优先,高密度类型靠后 */
export const ENTITY_TYPE_ORDER: Record<string, number> = {
  function: 0,
  method: 1,
  class: 2,
  struct: 3,
  interface: 4,
  enum: 5,
  type: 6,
  trait: 7,
  module: 8,
  constant: 9,
  variable: 10,
  property: 11,
  import: 12,
  section: 13,
};

export function entityTypeOrder(entityType: string): number {
  return ENTITY_TYPE_ORDER[entityType] ?? 50;
}

/** 默认折叠的高密度实体类型 */
export const COLLAPSED_ENTITY_TYPES = new Set(["property", "import", "section"]);

/** 展示序:类型序 → 行号 → 名称 */
export function compareEntitiesForDisplay(a: SemanticEntityRef, b: SemanticEntityRef): number {
  return (
    entityTypeOrder(a.entityType) - entityTypeOrder(b.entityType) ||
    a.startLine - b.startLine ||
    a.name.localeCompare(b.name)
  );
}

/** 实体显示名回退:无名时用类型名 */
export function entityDisplayName(entity: Pick<SemanticEntityRef, "name" | "entityType">): string {
  return entity.name || entity.entityType;
}

/** 去重:有 entityId 按 id,否则按 filePath+name+startLine */
export function dedupeEntityRefs(refs: SemanticEntityRef[]): SemanticEntityRef[] {
  const seen = new Set<string>();
  const out: SemanticEntityRef[] = [];
  for (const ref of refs) {
    const key = ref.entityId ?? `${ref.filePath}::${ref.name}::${ref.startLine}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(ref);
  }
  return out;
}

/** callers/refs 的分组结果拍平为去重后的关系项列表 */
export function flattenRelationGroups(groups: SemanticRelationGroup[]): SemanticEntityRef[] {
  return dedupeEntityRefs(groups.flatMap((group) => group.related));
}

// ── 影响分析小图(SemanticMiniGraph)布局辅助 ─────────────────────────────────

/** 小图单侧节点截取:最多 max 个,extra 为折叠进「+N」占位节点的数量 */
export function sliceGraphSide(
  list: SemanticEntityRef[],
  max = 8,
): { shown: SemanticEntityRef[]; extra: number } {
  const shown = list.slice(0, max);
  return { shown, extra: list.length - shown.length };
}

/** 小图节点标签截断(SVG 内定宽,超长省略) */
export function truncateGraphLabel(text: string, max = 22): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

// ── 会话级小型缓存(内存 Map,进程退出即清;不落 SQLite)──────────────────────

const fileEntitiesCache = new Map<string, SemanticFileEntitiesResult>();

export function cachedFileEntities(
  rootPath: string,
  filePath: string,
): SemanticFileEntitiesResult | undefined {
  return fileEntitiesCache.get(`${rootPath}::${filePath}`);
}

export function cacheFileEntities(
  rootPath: string,
  filePath: string,
  result: SemanticFileEntitiesResult,
) {
  // 上限防御:会话缓存不无限增长
  if (fileEntitiesCache.size > 200) fileEntitiesCache.clear();
  fileEntitiesCache.set(`${rootPath}::${filePath}`, result);
}

/** 使缓存失效:传 filePath 清单文件;省略则清整个 rootPath 项目 */
export function invalidateSemanticCache(rootPath: string, filePath?: string) {
  if (filePath !== undefined) {
    fileEntitiesCache.delete(`${rootPath}::${filePath}`);
    return;
  }
  const prefix = `${rootPath}::`;
  for (const key of fileEntitiesCache.keys()) {
    if (key.startsWith(prefix)) fileEntitiesCache.delete(key);
  }
}
