<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { FolderGit2, GitBranch, GitBranchPlus } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import WorktreeGraphCard from "@/components/git/WorktreeGraphCard.vue";
import {
  buildWorktreeTree,
  LABEL_W,
  layoutWorktreeTree,
  MAIN_H,
  PILL_W,
  RAIL_W,
  RAIL_X,
  ROW_GAP,
  ROW_H,
  stepLink,
  type WorktreeLayoutNode,
} from "@/components/git/worktree-graph";
import type { GitWorktree } from "@/types";

/**
 * worktree 树形图(d3 布局):主工作区整宽卡片置顶,linked worktree 按创建来源
 * 分支(base_branch)挂成树——来源是主工作区当前分支的直接挂在根下;来源未检出
 * 在任何工作区时生成虚线 "分支" 占位 pill,其子 worktree 在 pill 右侧竖向堆叠、
 * 与 pill 顶部对齐;来源是另一 worktree 检出分支的嵌套为其子节点。节点坐标与
 * 连线(L 形导轨线 + pill 分线贝塞尔)由 d3(hierarchy 递归布局 + link/
 * linkHorizontal)算出,卡片仍为绝对定位的 Vue DOM,宽度自适应容器。
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

const tree = computed(() => buildWorktreeTree(props.worktrees));
const layout = computed(() => (tree.value ? layoutWorktreeTree(tree.value) : null));
const hasChildren = computed(() => (tree.value?.children.length ?? 0) > 0);

/** 固定朝向箭头:尾部与连线末端相接,尖端贴齐子节点左缘 */
function arrowhead([x, y]: [number, number]) {
  return `M ${x - 9} ${y - 4.5} L ${x - 1} ${y} L ${x - 9} ${y + 4.5} Z`;
}

/** 卡片左缘缩进、右缘贴齐容器;占位分支 pill 宽度固定 */
function nodeStyle(n: WorktreeLayoutNode) {
  const base = { left: `${n.x}px`, top: `${n.y}px`, height: `${n.h}px` };
  return n.node.kind === "branch" ? { ...base, width: `${PILL_W}px` } : { ...base, right: "0px" };
}

// 空态(无 linked worktree):主卡下方一格,导轨短横线接虚线占位按钮,点击展开新建表单
const emptyTop = MAIN_H + ROW_GAP;
const emptyCenter = emptyTop + ROW_H / 2;
const emptyStub = stepLink(RAIL_X, MAIN_H, RAIL_W, emptyCenter);
const emptyDash = `M ${RAIL_W} ${emptyCenter} H ${RAIL_W + LABEL_W - 8}`;
const containerH = computed(() =>
  hasChildren.value ? (layout.value?.height ?? MAIN_H) : emptyTop + ROW_H,
);
</script>

<template>
  <div v-if="layout" class="relative" :style="{ height: `${containerH}px` }">
    <!-- 全部连线:d3 link(curveStepBefore)L 形路径 + linkHorizontal 贝塞尔 -->
    <svg class="absolute inset-0 h-full w-full text-border" fill="none" aria-hidden="true">
      <template v-for="l in layout.links" :key="l.key">
        <path :d="l.d" stroke="currentColor" stroke-width="1.5" />
        <path v-if="l.tip" :d="arrowhead(l.tip)" fill="currentColor" stroke="none" />
      </template>
      <template v-if="!hasChildren">
        <path :d="emptyStub" stroke="currentColor" stroke-width="1.5" />
        <path :d="emptyDash" stroke="currentColor" stroke-width="1.5" stroke-dasharray="4 4" />
      </template>
    </svg>

    <div v-for="n in layout.nodes" :key="n.node.key" class="absolute" :style="nodeStyle(n)">
      <!-- 主工作区:整宽卡片置顶 -->
      <div
        v-if="n.node.kind === 'main'"
        class="h-full rounded-md border px-3 py-2"
        :class="!activePath ? 'border-primary/60 bg-primary/5' : ''"
      >
        <div class="flex items-center gap-1.5">
          <FolderGit2 class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate text-sm font-medium">{{
            n.node.label || t("git.worktree.main")
          }}</span>
          <Badge variant="secondary" class="shrink-0 text-[10px]">{{
            t("git.worktree.main")
          }}</Badge>
          <Badge v-if="!activePath" class="shrink-0 text-[10px]">{{
            t("git.worktree.current")
          }}</Badge>
        </div>
        <p
          class="mt-1 truncate font-mono text-[11px] text-muted-foreground"
          :title="n.node.displayPath"
        >
          {{ n.node.displayPath }}
        </p>
      </div>

      <!-- 来源分支占位 pill(未检出在任何工作区),子 worktree 在其右侧竖向堆叠 -->
      <div
        v-else-if="n.node.kind === 'branch'"
        class="flex h-full items-center gap-1.5 rounded-md border border-dashed px-2.5 text-xs text-muted-foreground"
        :title="t('git.worktree.branchBase', { name: n.node.label })"
      >
        <GitBranch class="h-3.5 w-3.5 shrink-0" />
        <span class="truncate font-mono">{{ n.node.label }}</span>
      </div>

      <!-- linked worktree 卡片 -->
      <WorktreeGraphCard
        v-else
        :node="n.node"
        :active-path="activePath"
        :rebasing-path="rebasingPath"
        :rebase-interrupted-path="rebaseInterruptedPath"
        :aborting="aborting"
        @merge="emit('merge', $event)"
        @rebase="emit('rebase', $event)"
        @abort-rebase="emit('abortRebase', $event)"
        @remove="emit('remove', $event)"
      />
    </div>

    <!-- 空态:虚线占位按钮,点击展开新建表单 -->
    <button
      v-if="!hasChildren"
      type="button"
      class="absolute right-0 flex items-center justify-center gap-2 rounded-md border border-dashed text-xs text-muted-foreground transition-colors hover:border-primary/50 hover:text-primary"
      :style="{ left: `${RAIL_W + LABEL_W}px`, top: `${emptyTop}px`, height: `${ROW_H}px` }"
      @click="emit('create')"
    >
      <GitBranchPlus class="h-3.5 w-3.5" />
      {{ t("git.worktree.createFirst") }}
    </button>
  </div>
</template>
