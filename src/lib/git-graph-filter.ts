/** 分支列表的最小形状(local/remote 名列表) */
export interface BranchScope {
  local: string[];
  remote: string[];
}

/** 分支列表中是否存在该分支(本地或远程) */
export function hasBranch(scope: BranchScope, name: string): boolean {
  return scope.local.includes(name) || scope.remote.includes(name);
}

export interface FilterTargetResolution {
  /** 校正后的筛选目标;空串表示回退到当前检出分支 */
  branch: string;
  /** 被清除的失效分支名(空表示无需校正) */
  dropped: string | null;
}

/**
 * 校正「当前分支」筛选目标。
 *
 * 刷新远程引用(尤其 `fetch --prune`)后,原先选中的远程分支可能已被删除。
 * 继续用它作为 `git_graph_log` 的修订范围会报 bad revision,既拿不到日志,
 * 也让旧分支与旧图谱滞留在界面上。此时清除失效选择:置空后由调用方回退到
 * 当前检出分支,当前检出分支也不在列表时(空仓库等)按「全部分支」处理。
 *
 * 非「当前分支」筛选(全部分支)不涉及修订范围,原样返回。
 */
export function resolveFilterTarget(
  scope: BranchScope,
  filterValue: string,
  selectedBranch: string,
): FilterTargetResolution {
  if (filterValue !== "current" || !selectedBranch) {
    return { branch: selectedBranch, dropped: null };
  }
  if (hasBranch(scope, selectedBranch)) {
    return { branch: selectedBranch, dropped: null };
  }
  // 已删除的分支清空选择:回退到当前检出分支,仍无效时由 graphParams 落到全部分支
  return { branch: "", dropped: selectedBranch };
}
