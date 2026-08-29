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
