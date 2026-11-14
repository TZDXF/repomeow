import type { GitBranches } from "@/types";
import {
  resolveFilterTarget,
  type BranchScope,
  type FilterTargetResolution,
} from "./git-graph-filter";

export interface GraphLoadPreparation {
  /** 最新分支列表;请求失败时为 null(沿用调用方旧列表) */
  branches: GitBranches | null;
  /** 筛选目标校正结果 */
  correction: FilterTargetResolution;
}

/**
 * 提交图谱加载前的准备:先取分支列表,再据此校验筛选目标。
 *
 * 顺序与降级语义是缺陷 TZD-97/P2 的修复要点:
 * - 分支列表必须先落库。若与 `git_graph_log` 绑在同一个 Promise.all 里,日志的坏修订
 *   (如已被 prune 的远程分支)会让新列表一起拿不到,侧栏继续显示已删除分支与旧图谱。
 * - 列表请求失败不阻断日志:返回 null,调用方沿用旧列表继续画图。
 * - 列表到手后校验筛选目标:选中分支已消失时清空选择,调用方据此回退到当前检出分支。
 */
export async function prepareGraphLoad(
  listBranches: () => Promise<GitBranches>,
  fallbackScope: BranchScope,
  filterValue: string,
  selectedBranch: string,
): Promise<GraphLoadPreparation> {
  let branches: GitBranches | null = null;
  try {
    branches = await listBranches();
  } catch {
    branches = null;
  }
  return {
    branches,
    correction: resolveFilterTarget(branches ?? fallbackScope, filterValue, selectedBranch),
  };
}
