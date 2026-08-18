<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import {
  ArrowUpToLine,
  FolderGit2,
  GitBranch,
  GitBranchPlus,
  GitMerge,
  Loader2,
  Trash2,
  Undo2,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { displayRelativeTo } from "@/lib/path";
import type { GitWorktree } from "@/types";

/**
 * worktree 树形图:主工作区为根节点,各 linked worktree 为子节点纵向排列,
 * 中间一条固定宽度的 SVG 连线路用贝塞尔曲线表达"共享同一仓库、各自检出不同
 * 分支"的结构。子节点卡片固定行高,连接线路径由行号直接算出,无需测量 DOM。
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

/** 行几何:与模板里的 h-16 / 固定 gap 保持一致 */
const ROW_H = 64;
const ROW_GAP = 12;
const LANE_W = 56;

const mainWorktree = computed(() => props.worktrees.find((w) => w.is_main));
const linked = computed(() => props.worktrees.filter((w) => !w.is_main));

const rowCount = computed(() => Math.max(linked.value.length, 1));
const stackHeight = computed(() => rowCount.value * ROW_H + (rowCount.value - 1) * ROW_GAP);
/** 根节点与连线路均相对子节点栈垂直居中,根节点引出点在连线左缘中点 */
const rootY = computed(() => stackHeight.value / 2);
function childY(i: number) {
  return i * (ROW_H + ROW_GAP) + ROW_H / 2;
}
/** 根 -> 子节点的三次贝塞尔;终点切线水平,末端接固定朝向的箭头 */
function connector(i: number) {
  const y = childY(i);
  const mid = LANE_W / 2;
  return `M 0 ${rootY.value} C ${mid} ${rootY.value}, ${mid} ${y}, ${LANE_W - 8} ${y}`;
}
function arrowhead(i: number) {
  const y = childY(i);
  return `M ${LANE_W - 9} ${y - 4.5} L ${LANE_W - 1} ${y} L ${LANE_W - 9} ${y + 4.5} Z`;
}

function displayPath(w: GitWorktree) {
  return displayRelativeTo(mainWorktree.value?.path ?? "", w.path);
}

/** 该行是否可合并/变基:游离 HEAD 无分支可操作 */
function hasBranch(w: GitWorktree) {
  return !!w.branch;
}
</script>

<template>
  <div class="flex items-center">
    <!-- 根节点:主工作区 -->
    <div class="w-40 shrink-0">
      <div
        class="rounded-md border px-3 py-2"
        :class="!activePath ? 'border-primary/60 bg-primary/5' : ''"
      >
        <div class="flex items-center gap-1.5">
          <FolderGit2 class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate text-sm font-medium">
            {{ mainWorktree?.branch ?? t("git.worktree.main") }}
          </span>
        </div>
        <div class="mt-1 flex items-center gap-1">
          <Badge variant="secondary" class="text-[10px]">{{ t("git.worktree.main") }}</Badge>
          <Badge v-if="!activePath" class="text-[10px]">{{ t("git.worktree.current") }}</Badge>
        </div>
        <p
          v-if="mainWorktree"
          class="mt-1 truncate font-mono text-[11px] text-muted-foreground"
          :title="mainWorktree.path"
        >
          {{ mainWorktree.path }}
        </p>
      </div>
    </div>

    <!-- 连线路:根节点 -> 各子节点 -->
    <svg
      :width="LANE_W"
      :height="stackHeight"
      class="shrink-0 text-border"
      fill="none"
      aria-hidden="true"
    >
      <template v-for="(w, i) in linked" :key="w.path">
        <path :d="connector(i)" stroke="currentColor" stroke-width="1.5" />
        <path :d="arrowhead(i)" fill="currentColor" stroke="none" />
      </template>
      <template v-if="!linked.length">
        <path
          :d="`M 0 ${rootY} L ${LANE_W - 8} ${rootY}`"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-dasharray="4 4"
        />
        <path :d="arrowhead(0)" fill="currentColor" stroke="none" opacity="0.5" />
      </template>
    </svg>

    <!-- 子节点栈:各 linked worktree -->
    <div class="flex min-w-0 flex-1 flex-col" :style="{ gap: `${ROW_GAP}px` }">
      <div
        v-for="w in linked"
        :key="w.path"
        class="flex h-16 items-center gap-2 rounded-md border px-3"
        :class="activePath === w.path ? 'border-primary/60 bg-primary/5' : ''"
      >
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-1.5">
            <GitBranch class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            <span class="truncate text-sm font-medium">{{ w.branch ?? w.head.slice(0, 7) }}</span>
            <Badge v-if="w.detached" variant="outline" class="shrink-0 text-[10px]">
              {{ t("git.worktree.detached") }}
            </Badge>
            <Badge v-if="activePath === w.path" class="shrink-0 text-[10px]">
              {{ t("git.worktree.current") }}
            </Badge>
          </div>
          <p class="mt-0.5 truncate font-mono text-[11px] text-muted-foreground" :title="w.path">
            {{ displayPath(w) }}
          </p>
        </div>
        <div class="flex shrink-0 items-center gap-0.5">
          <Button
            variant="ghost"
            size="xs"
            :disabled="!hasBranch(w)"
            :title="t('git.worktree.mergeBack')"
            @click="emit('merge', w)"
          >
            <GitMerge class="h-3.5 w-3.5" />
          </Button>
          <Button
            v-if="rebaseInterruptedPath === w.path"
            variant="ghost"
            size="xs"
            class="text-amber-600"
            :disabled="aborting"
            :title="t('git.worktree.abortRebase')"
            @click="emit('abortRebase', w)"
          >
            <Undo2 class="h-3.5 w-3.5" />
          </Button>
          <Button
            v-else
            variant="ghost"
            size="xs"
            :disabled="!hasBranch(w) || rebasingPath === w.path"
            :title="t('git.worktree.rebase')"
            @click="emit('rebase', w)"
          >
            <Loader2 v-if="rebasingPath === w.path" class="h-3.5 w-3.5 animate-spin" />
            <ArrowUpToLine v-else class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="xs"
            class="text-destructive"
            :title="t('git.worktree.remove')"
            @click="emit('remove', w)"
          >
            <Trash2 class="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <!-- 空态:虚线占位节点,点击展开新建表单 -->
      <button
        v-if="!linked.length"
        type="button"
        class="flex h-16 items-center justify-center gap-2 rounded-md border border-dashed text-xs text-muted-foreground transition-colors hover:border-primary/50 hover:text-primary"
        @click="emit('create')"
      >
        <GitBranchPlus class="h-3.5 w-3.5" />
        {{ t("git.worktree.createFirst") }}
      </button>
    </div>
  </div>
</template>
