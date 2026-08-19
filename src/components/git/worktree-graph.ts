import { curveStepBefore, hierarchy, link, linkHorizontal, type HierarchyNode } from "d3";
import { displayRelativeTo } from "@/lib/path";
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
}

/** 行几何:与卡片 h-10 / 固定 gap 保持一致;主卡为两行内容,更高 */
export const ROW_H = 40;
export const MAIN_H = 64;
export const ROW_GAP = 16;
/** 父节点竖向导轨:导轨区宽度与纵线 x 位置 */
export const RAIL_W = 32;
export const RAIL_X = 16;
/** 标签列宽 = 占位分支 pill(160)+ 分线连线路(40) */
export const PILL_W = 160;
export const LANE_W = 40;
export const LABEL_W = PILL_W + LANE_W;
/** worktree 卡片相对父卡片左缘的缩进(导轨区 + 标签列) */
export const CARD_INDENT = RAIL_W + LABEL_W;

/** "origin/team/x" -> "team/x";不含 / 原样返回(用于远程引用与本地分支名互配) */
function shortName(branch: string) {
  const i = branch.indexOf("/");
  return i === -1 ? branch : branch.slice(i + 1);
}

/**
 * 按创建来源分支(base_branch)把 linked worktree 挂成树:来源是主工作区当前分支的
 * 直接挂在根下;来源未检出在任何工作区(远程引用或本地分支)时生成 "branch" 占位
 * 节点;来源是另一 worktree 检出分支的嵌套为其子节点;A/B 互为来源成环时后挂接者
 * 回退到根。
 */
export function buildWorktreeTree(worktrees: GitWorktree[]): WorktreeTreeNode | null {
  const main = worktrees.find((w) => w.is_main);
  if (!main) {
    return null;
  }
  const linked = worktrees.filter((w) => !w.is_main);
  const root: WorktreeTreeNode = {
    key: `main:${main.path}`,
    kind: "main",
    worktree: main,
    label: main.branch ?? "",
    displayPath: main.path,
    children: [],
  };
  const parentOf = new Map<WorktreeTreeNode, WorktreeTreeNode>();
  const attach = (node: WorktreeTreeNode, parent: WorktreeTreeNode) => {
    parent.children.push(node);
    parentOf.set(node, parent);
  };
  // 分支名 -> worktree 节点(同一分支不会同时检出在两个工作区)
  const byBranch = new Map<string, WorktreeTreeNode>();
  const byPath = new Map<string, WorktreeTreeNode>();
  for (const w of linked) {
    const n: WorktreeTreeNode = {
      key: `wt:${w.path}`,
      kind: "worktree",
      worktree: w,
      label: w.branch ?? w.head.slice(0, 7),
      displayPath: displayRelativeTo(main.path, w.path),
      children: [],
    };
    byPath.set(w.path, n);
    if (w.branch) {
      byBranch.set(w.branch, n);
    }
  }
  /** 沿已挂接的父链检查会否成环(A 基于 B、B 又基于 A 时后挂接者回退到根) */
  const createsCycle = (parent: WorktreeTreeNode, self: WorktreeTreeNode) => {
    let p: WorktreeTreeNode | undefined = parent;
    while (p) {
      if (p === self) {
        return true;
      }
      p = parentOf.get(p);
    }
    return false;
  };
  const branchNodes = new Map<string, WorktreeTreeNode>();
  for (const w of linked) {
    const node = byPath.get(w.path)!;
    const base = w.base_branch?.trim();
    let parent = root;
    if (base && base !== w.branch) {
      const cands = [...new Set([base, shortName(base)])];
      const onMain = !!main.branch && cands.includes(main.branch);
      if (!onMain) {
        const exactHit = byBranch.get(base);
        const shortHit = byBranch.get(shortName(base));
        // 短名互配命中的工作区若自身也来自同一 base(如 base 写 origin/zc-dev,命中
        // 的 zc-dev 工作区同样从 origin/zc-dev 创建),两者是兄弟而非父子,应共同挂到
        // base 的占位分支节点下
        const hit =
          exactHit && exactHit !== node
            ? exactHit
            : shortHit && shortHit !== node && shortHit.worktree?.base_branch?.trim() !== base
              ? shortHit
              : undefined;
        if (hit && !createsCycle(hit, node)) {
          parent = hit;
        } else if (!hit) {
          // 来源分支未检出在任何工作区:同名来源共享一个占位分支节点
          let bn = branchNodes.get(base);
          if (!bn) {
            bn = {
              key: `br:${base}`,
              kind: "branch",
              worktree: null,
              label: base,
              displayPath: "",
              children: [],
            };
            branchNodes.set(base, bn);
            attach(bn, root);
          }
          parent = bn;
        }
      }
    }
    attach(node, parent);
  }
  return root;
}

/** 布局后的节点:绝对定位的左/上/高(宽度由容器决定,卡片右缘贴齐容器) */
export interface WorktreeLayoutNode {
  node: WorktreeTreeNode;
  x: number;
  y: number;
  h: number;
}

export interface WorktreeLayoutLink {
  key: string;
  d: string;
  /** 箭头尖端坐标(贴齐子节点左缘);连线末端已退到箭头尾部。null = 无箭头(占位分支 pill) */
  tip: [number, number] | null;
}

/** 连线末端相对子节点左缘的退让(箭头长度) */
const ARROW = 8;

interface LinkPoints {
  source: [number, number];
  target: [number, number];
}

/**
 * d3 link + curveStepBefore:自父节点下沿竖直下行、末段水平接入子节点左缘中心。
 * 同一父节点的各子连线竖直段重叠,视觉上即一条导轨加若干横向短接线。
 */
const elbow = link<LinkPoints, [number, number]>(curveStepBefore)
  .x((p) => p[0])
  .y((p) => p[1]);

/** d3 linkHorizontal(默认 curveBumpX):占位分支 pill 右缘 -> 子卡左缘的三次贝塞尔 */
const lane = linkHorizontal<LinkPoints, [number, number]>()
  .x((p) => p[0])
  .y((p) => p[1]);

/** 单段 L 形连线(父下沿 stub -> 子左缘中心) */
export function stepLink(sx: number, sy: number, tx: number, ty: number): string {
  return elbow({ source: [sx, sy], target: [tx, ty] }) ?? "";
}

/**
 * d3 hierarchy 递归布局,复刻原竖向导轨几何:主卡/卡片独占一行,子节点区块与父
 * 节点下沿间隔 ROW_GAP、按 CARD_INDENT 缩进;占位分支 pill 的子 worktree 向右引出
 * (pill.x + LABEL_W)竖向堆叠、与 pill 顶部对齐,pill 块高取 pill 行高与子卡栈高
 * 的较大值。卡片宽度由容器决定,故布局无需知悉容器宽度。返回节点绝对坐标与连线路径。
 */
export function layoutWorktreeTree(root: WorktreeTreeNode): {
  nodes: WorktreeLayoutNode[];
  links: WorktreeLayoutLink[];
  height: number;
} {
  const nodes: WorktreeLayoutNode[] = [];
  const links: WorktreeLayoutLink[] = [];

  /** 放置 h 及其子树,返回该节点块总高 */
  const place = (h: HierarchyNode<WorktreeTreeNode>, x: number, y: number): number => {
    const n = h.data;
    const rowH = n.kind === "main" ? MAIN_H : ROW_H;
    nodes.push({ node: n, x, y, h: rowH });
    const kids = h.children ?? [];
    if (!kids.length) {
      return rowH;
    }

    if (n.kind === "branch") {
      // 子 worktree 在 pill 右侧竖向堆叠,首个子卡与 pill 顶部对齐(不垂直居中)
      let cy = y;
      for (const c of kids) {
        const ch = place(c, x + LABEL_W, cy);
        links.push({
          key: `${n.key}->${c.data.key}`,
          d:
            lane({
              source: [x + PILL_W, y + ROW_H / 2],
              target: [x + LABEL_W - ARROW, cy + ROW_H / 2],
            }) ?? "",
          tip: [x + LABEL_W, cy + ROW_H / 2],
        });
        cy += ch + ROW_GAP;
      }
      return Math.max(rowH, cy - ROW_GAP - y);
    }

    // 子节点区块与父节点下沿间隔 ROW_GAP:worktree 子卡缩进整个标签列,占位分支 pill 只缩进导轨区
    let cy = y + rowH + ROW_GAP;
    for (const c of kids) {
      const isBranch = c.data.kind === "branch";
      const cx = isBranch ? x + RAIL_W : x + CARD_INDENT;
      const ch = place(c, cx, cy);
      links.push({
        key: `${n.key}->${c.data.key}`,
        d: stepLink(x + RAIL_X, y + rowH, isBranch ? cx : cx - ARROW, cy + ROW_H / 2),
        tip: isBranch ? null : [cx, cy + ROW_H / 2],
      });
      cy += ch + ROW_GAP;
    }
    return cy - ROW_GAP - y;
  };

  const height = place(
    hierarchy(root, (n) => n.children),
    0,
    0,
  );
  return { nodes, links, height };
}
