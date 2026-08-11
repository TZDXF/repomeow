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
 * toRow === 已加载节点数 表示父提交尚未加载(流式加载中),画到当前内容底部
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

export interface GraphLayouter {
  /** 追加一批拓扑序提交(子提交先于父提交,后端 --topo-order 保证) */
  push: (commits: GitGraphCommit[]) => void;
  reset: () => void;
  /** 已布局节点(push 就地追加,数组引用不变,视图需自行触发更新) */
  readonly nodes: GraphNodeLayout[];
  /** 已加载节点数对应的连线;父提交尚未加载的连线 toRow 指向当前内容底部 */
  edges: () => GraphEdgeLayout[];
  /** 布局期间出现过的最大泳道数(决定图形区宽度) */
  readonly laneCount: number;
}

/**
 * 增量泳道布局器:状态可持续追加,配合流式加载避免每批全量重算。
 * 第一父提交接管当前泳道(线性历史渲染为直线),其余父提交分配空闲/新泳道
 */
export function createGraphLayouter(): GraphLayouter {
  // lanes[i] = 该泳道正在等待的 commit hash;null = 空闲可复用
  const lanes: (string | null)[] = [];
  const nodes: GraphNodeLayout[] = [];
  // 待解析的连线:fromRow/fromLane/toLane 在子提交处即可确定,toRow 待父提交到达后解析
  const pending: { fromRow: number; fromLane: number; toLane: number; parent: string }[] = [];
  const rowByHash = new Map<string, number>();
  let maxLanes = 0;

  const allocLane = (): number => {
    const free = lanes.indexOf(null);
    if (free !== -1) {
      return free;
    }
    lanes.push(null);
    return lanes.length - 1;
  };

  return {
    nodes,
    get laneCount() {
      return Math.max(maxLanes, 1);
    },
    push(commits: GitGraphCommit[]) {
      for (const commit of commits) {
        const row = nodes.length;
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
        rowByHash.set(commit.hash, row);

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
        maxLanes = Math.max(maxLanes, lanes.length);
      }
    },
    edges(): GraphEdgeLayout[] {
      return pending.map((e) => ({
        fromRow: e.fromRow,
        fromLane: e.fromLane,
        toLane: e.toLane,
        toRow: rowByHash.get(e.parent) ?? nodes.length,
        color: e.toLane,
      }));
    },
    reset() {
      lanes.length = 0;
      nodes.length = 0;
      pending.length = 0;
      rowByHash.clear();
      maxLanes = 0;
    },
  };
}

/**
 * 泳道布局:输入必须为拓扑序(子提交先于父提交,后端 --topo-order 保证)。
 * 一次性计算版本(增量版本见 createGraphLayouter)
 */
export function computeGraphLayout(commits: GitGraphCommit[]): GraphLayout {
  const layouter = createGraphLayouter();
  layouter.push(commits);
  return { nodes: layouter.nodes, edges: layouter.edges(), laneCount: layouter.laneCount };
}
