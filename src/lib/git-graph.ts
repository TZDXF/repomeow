import type { GitGraphCommit } from "@/types";

/** 图谱中一个提交节点的布局结果(color 为泳道配色下标) */
export interface GraphNodeLayout {
  commit: GitGraphCommit;
  row: number;
  lane: number;
  color: number;
}

/**
 * 一条父子连线(row/lane 坐标,由视图换算为像素)。
 * toRow === 提交总数 表示父提交在加载窗口外,画到列表底部
 */
export interface GraphEdgeLayout {
  fromRow: number;
  fromLane: number;
  toRow: number;
  toLane: number;
  color: number;
}

export interface GraphLayout {
  nodes: GraphNodeLayout[];
  edges: GraphEdgeLayout[];
  /** 布局期间出现过的最大泳道数(决定图形区宽度) */
  laneCount: number;
}

/** 泳道配色(按 color 下标取模) */
export const LANE_PALETTE = [
  "#3b82f6", // blue
  "#22c55e", // green
  "#f97316", // orange
  "#a855f7", // purple
  "#ef4444", // red
  "#14b8a6", // teal
  "#eab308", // yellow
  "#ec4899", // pink
];

export function laneColor(color: number): string {
  return LANE_PALETTE[color % LANE_PALETTE.length];
}

/**
 * 泳道布局:输入必须为拓扑序(子提交先于父提交,后端 --topo-order 保证)。
 * 第一父提交接管当前泳道(线性历史渲染为直线),其余父提交分配空闲/新泳道
 */
export function computeGraphLayout(commits: GitGraphCommit[]): GraphLayout {
  // lanes[i] = 该泳道正在等待的 commit hash;null = 空闲可复用
  const lanes: (string | null)[] = [];
  const nodes: GraphNodeLayout[] = [];
  // 待解析的连线:fromRow/fromLane/toLane 第一遍即可确定,toRow 第二遍补齐
  const pending: { fromRow: number; fromLane: number; toLane: number; parent: string }[] = [];
  let laneCount = 0;

  const allocLane = (): number => {
    const free = lanes.indexOf(null);
    if (free !== -1) {
      return free;
    }
    lanes.push(null);
    return lanes.length - 1;
  };

  commits.forEach((commit, row) => {
    let lane = lanes.indexOf(commit.hash);
    if (lane === -1) {
      lane = allocLane();
    }
    // 关闭其余等待同一提交的重复泳道(多个子提交指向同一父提交时产生),
    // 并把指向这些泳道的连线重定向到节点实际所在泳道
    for (let i = 0; i < lanes.length; i++) {
      if (i !== lane && lanes[i] === commit.hash) {
        lanes[i] = null;
        for (const e of pending) {
          if (e.parent === commit.hash && e.toLane === i) {
            e.toLane = lane;
          }
        }
      }
    }
    nodes.push({ commit, row, lane, color: lane });

    const [first, ...rest] = commit.parents;
    lanes[lane] = first ?? null;
    if (first) {
      pending.push({ fromRow: row, fromLane: lane, toLane: lane, parent: first });
    }
    for (const p of rest) {
      // 已有泳道在等待该父提交则复用,否则分配新泳道
      let target = lanes.indexOf(p);
      if (target === -1) {
        target = allocLane();
        lanes[target] = p;
      }
      pending.push({ fromRow: row, fromLane: lane, toLane: target, parent: p });
    }
    laneCount = Math.max(laneCount, lanes.length);
  });

  const rowByHash = new Map<string, number>();
  nodes.forEach((n) => rowByHash.set(n.commit.hash, n.row));

  const edges: GraphEdgeLayout[] = pending.map((e) => ({
    fromRow: e.fromRow,
    fromLane: e.fromLane,
    toLane: e.toLane,
    toRow: rowByHash.get(e.parent) ?? commits.length,
    color: e.toLane,
  }));

  return { nodes, edges, laneCount: Math.max(laneCount, 1) };
}
