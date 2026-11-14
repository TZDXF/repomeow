/**
 * 提交图谱「点记录」与右侧详情的状态机(纯函数,便于单测)。
 *
 * 交互约定:
 * - 点另一条记录:选中它并展开详情
 * - 点已选中的记录且详情展开:不取消选中,只收起详情(保留高亮与折叠窄条)
 * - 点已选中的记录且详情已收起:再次展开(窄条/记录都是展开入口)
 */
export interface DetailToggleState {
  selectedHash: string | null;
  detailOpen: boolean;
}

export function toggleDetailByCommit(current: DetailToggleState, hash: string): DetailToggleState {
  if (current.selectedHash === hash) {
    return { selectedHash: hash, detailOpen: !current.detailOpen };
  }
  return { selectedHash: hash, detailOpen: true };
}

/** 定位/搜索等明确「看这条提交」的动作:选中并展开详情 */
export function revealCommit(hash: string): DetailToggleState {
  return { selectedHash: hash, detailOpen: true };
}
