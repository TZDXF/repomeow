import type { GitWorktree } from "@/types";

/** worktree 树节点:main = 主工作区;worktree = 链接工作区;branch = 来源分支占位(未检出在任何工作区) */
export interface WorktreeTreeNode {
  key: string;
  kind: "main" | "worktree" | "branch";
  worktree: GitWorktree | null;
  /** 展示名:main/worktree 为检出分支(或 HEAD 短哈希),branch 为来源分支名 */
  label: string;
  /** worktree/main 的显示路径(worktree 为相对主工作区的路径) */
  displayPath: string;
  children: WorktreeTreeNode[];
  /** 节点块总高(px):worktree/main = 卡片行 + 下方子导轨区;branch = 右侧子卡栈高 */
  totalH: number;
  /** 块在父节点子栈内的顶部偏移(父节点量高时写入) */
  top: number;
  /** 父导轨横线接入点在块内的纵向偏移(恒为卡片/pill 中心:顶部对齐,非垂直居中) */
  stubY: number;
}

/** 行几何:与模板里的 h-10 / 固定 gap 保持一致 */
export const ROW_H = 40;
export const ROW_GAP = 16;
/** 父节点竖向导轨:导轨区宽度与纵线 x 位置 */
export const RAIL_W = 32;
export const RAIL_X = 16;
/** 标签列宽 = 占位分支 pill(160)+ 分线连线路(40) */
export const PILL_W = 160;
export const LANE_W = 40;
export const LABEL_W = PILL_W + LANE_W;
