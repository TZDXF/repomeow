<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { FolderGit2, GitBranchPlus } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import WorktreeGraphRail from "@/components/git/WorktreeGraphRail.vue";
import {
  LABEL_W,
  RAIL_W,
  RAIL_X,
  ROW_GAP,
  ROW_H,
  type WorktreeTreeNode,
} from "@/components/git/worktree-graph";
import { displayRelativeTo } from "@/lib/path";
import type { GitWorktree } from "@/types";

/**
 * worktree 树形图(竖向导轨式):主工作区整宽卡片置顶,其下一条竖向导轨,
 * 各 linked worktree 按创建来源分支(base_branch)挂接——来源是主工作区当前分支的
 * 直接经横线接入导轨;来源未检出在任何工作区(远程引用或本地分支)时生成虚线
 * "分支"占位节点挂在导轨上(分支向下排列),其子 worktree 在右侧竖向堆叠(向右
 * 引出);来源是另一 worktree 检出分支的嵌套在该 worktree 卡片下方的缩进导轨里。
 * 节点卡片固定行高,导轨/连线由子树高度直接算出,无需测量 DOM。
 * 合并/变基/删除操作经事件交给 WorktreePanel 处理。
 */
const { t } = useI18n();
const props = defineProps<{
  worktrees: GitWorktree[];
  /** 详情页当前选中的工作区路径,null = 主工作区 */
  activePath: string | null;
  rebasingPath: string | null;
  rebaseInterruptedPath: string | null;
  aborting: boolean;
}>();
const emit = defineEmits<{
  merge: [w: GitWorktree];
  rebase: [w: GitWorktree];
  abortRebase: [w: GitWorktree];
  remove: [w: GitWorktree];
  create: [];
}>();

const mainWorktree = computed(() => props.worktrees.find((w) => w.is_main));
const linked = computed(() => props.worktrees.filter((w) => !w.is_main));

/** "origin/team/x" -> "team/x";不含 / 原样返回(用于远程引用与本地分支名互配) */
function shortName(branch: string) {
  const i = branch.indexOf("/");
  return i === -1 ? branch : branch.slice(i + 1);
}

const tree = computed<WorktreeTreeNode | null>(() => {
  const main = mainWorktree.value;
  if (!main) {
    return null;
  }
  const root: WorktreeTreeNode = {
    key: `main:${main.path}`,
    kind: "main",
    worktree: main,
    label: main.branch ?? "",
    displayPath: main.path,
    children: [],
    totalH: ROW_H,
    top: 0,
    stubY: 0,
  };
  const parentOf = new Map<WorktreeTreeNode, WorktreeTreeNode>();
  const attach = (node: WorktreeTreeNode, parent: WorktreeTreeNode) => {
    parent.children.push(node);
    parentOf.set(node, parent);
  };
  // 分支名 -> worktree 节点(同一分支不会同时检出在两个工作区)
  const byBranch = new Map<string, WorktreeTreeNode>();
  const byPath = new Map<string, WorktreeTreeNode>();
  for (const w of linked.value) {
    const n: WorktreeTreeNode = {
      key: `wt:${w.path}`,
      kind: "worktree",
      worktree: w,
      label: w.branch ?? w.head.slice(0, 7),
      displayPath: displayRelativeTo(main.path, w.path),
      children: [],
      totalH: ROW_H,
      top: 0,
      stubY: 0,
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
  for (const w of linked.value) {
    const node = byPath.get(w.path)!;
    const base = w.base_branch?.trim();
    let parent = root;
    if (base && base !== w.branch) {
      const cands = [...new Set([base, shortName(base)])];
      const onMain = !!main.branch && cands.includes(main.branch);
      if (!onMain) {
        const hit = cands.map((c) => byBranch.get(c)).find((p) => p && p !== node);
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
              totalH: ROW_H,
              top: 0,
              stubY: 0,
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
  // 自底向上量各节点块高,并记录每个子节点在父节点子栈内的顶部偏移与导轨接入点
  const measure = (n: WorktreeTreeNode): number => {
    if (!n.children.length) {
      n.totalH = ROW_H;
    } else {
      let y = 0;
      for (const c of n.children) {
        measure(c);
        c.top = y;
        y += c.totalH + ROW_GAP;
      }
      const sectionH = y - ROW_GAP;
      n.totalH = n.kind === "branch" ? Math.max(ROW_H, sectionH) : ROW_H + sectionH;
    }
    // 导轨横线一律接在卡片/pill 中心(分支 pill 与其子卡栈顶部对齐,不垂直居中)
    n.stubY = ROW_H / 2;
    return n.totalH;
  };
  measure(root);
  return root;
});
</script>

<template>
  <div v-if="tree">
    <!-- 主工作区:整宽卡片置顶,竖向导轨从其下沿引出 -->
    <div
      class="rounded-md border px-3 py-2"
      :class="!activePath ? 'border-primary/60 bg-primary/5' : ''"
    >
      <div class="flex items-center gap-1.5">
        <FolderGit2 class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span class="truncate text-sm font-medium">{{ tree.label || t("git.worktree.main") }}</span>
        <Badge variant="secondary" class="shrink-0 text-[10px]">{{ t("git.worktree.main") }}</Badge>
        <Badge v-if="!activePath" class="shrink-0 text-[10px]">{{
          t("git.worktree.current")
        }}</Badge>
      </div>
      <p
        class="mt-1 truncate font-mono text-[11px] text-muted-foreground"
        :title="tree.displayPath"
      >
        {{ tree.displayPath }}
      </p>
    </div>

    <WorktreeGraphRail
      v-if="tree.children.length"
      :nodes="tree.children"
      :active-path="activePath"
      :rebasing-path="rebasingPath"
      :rebase-interrupted-path="rebaseInterruptedPath"
      :aborting="aborting"
      @merge="emit('merge', $event)"
      @rebase="emit('rebase', $event)"
      @abort-rebase="emit('abortRebase', $event)"
      @remove="emit('remove', $event)"
    />

    <!-- 空态:导轨短横线接虚线占位节点,点击展开新建表单 -->
    <div v-else class="relative" :style="{ paddingLeft: `${RAIL_W}px` }">
      <svg
        :width="RAIL_W"
        :height="ROW_H"
        class="absolute left-0 top-0 text-border"
        fill="none"
        aria-hidden="true"
      >
        <path
          :d="`M ${RAIL_X} 0 V ${ROW_H / 2} H ${RAIL_W}`"
          stroke="currentColor"
          stroke-width="1.5"
        />
      </svg>
      <div class="flex">
        <svg
          :width="LABEL_W"
          :height="ROW_H"
          class="shrink-0 text-border"
          fill="none"
          aria-hidden="true"
        >
          <path
            :d="`M 0 ${ROW_H / 2} H ${LABEL_W - 8}`"
            stroke="currentColor"
            stroke-width="1.5"
            stroke-dasharray="4 4"
          />
        </svg>
        <button
          type="button"
          class="flex h-10 min-w-0 flex-1 items-center justify-center gap-2 rounded-md border border-dashed text-xs text-muted-foreground transition-colors hover:border-primary/50 hover:text-primary"
          @click="emit('create')"
        >
          <GitBranchPlus class="h-3.5 w-3.5" />
          {{ t("git.worktree.createFirst") }}
        </button>
      </div>
    </div>
  </div>
</template>
