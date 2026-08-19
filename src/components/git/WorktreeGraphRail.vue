<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { GitBranch } from "@lucide/vue";
import WorktreeGraphCard from "@/components/git/WorktreeGraphCard.vue";
import {
  LABEL_W,
  LANE_W,
  PILL_W,
  RAIL_W,
  RAIL_X,
  ROW_GAP,
  ROW_H,
  type WorktreeTreeNode,
} from "@/components/git/worktree-graph";
import type { GitWorktree } from "@/types";

/**
 * 竖向导轨区(递归):父节点卡片下沿引出一条竖线,各子节点经横向连线向右接入。
 * worktree 子节点 = 横线 + 卡片(子树导轨缩进在卡片下方);branch 占位子节点 =
 * pill + 分线连线路,其子 worktree 卡片在右侧竖向堆叠。连线两端均贴齐卡片边缘。
 */
const { t } = useI18n();
const props = defineProps<{
  nodes: WorktreeTreeNode[];
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
}>();

const last = computed(() => props.nodes[props.nodes.length - 1]);
const sectionH = computed(() => (last.value ? last.value.top + last.value.totalH : 0));
/** 竖线终止于末子节点的接入点(其后不再延伸) */
const lastStubY = computed(() => (last.value ? last.value.top + last.value.stubY : 0));

/** 横线末端/分线末端的固定朝向箭头 */
function arrowhead(x: number, y: number) {
  return `M ${x - 9} ${y - 4.5} L ${x - 1} ${y} L ${x - 9} ${y + 4.5} Z`;
}

/** 占位分支 pill -> 各子 worktree 卡片的三次贝塞尔;pill 与首个子卡片顶部对齐,起点即 pill 中心 */
function laneConnector(child: WorktreeTreeNode) {
  const y0 = ROW_H / 2;
  const y1 = child.top + ROW_H / 2;
  const mid = LANE_W / 2;
  return `M 0 ${y0} C ${mid} ${y0}, ${mid} ${y1}, ${LANE_W - 8} ${y1}`;
}
</script>

<template>
  <div class="relative" :style="{ paddingLeft: `${RAIL_W}px` }">
    <!-- 竖向导轨 + 各子节点横向接入线 -->
    <svg
      :width="RAIL_W"
      :height="sectionH"
      class="absolute left-0 top-0 text-border"
      fill="none"
      aria-hidden="true"
    >
      <path :d="`M ${RAIL_X} 0 V ${lastStubY}`" stroke="currentColor" stroke-width="1.5" />
      <path
        v-for="c in nodes"
        :key="c.key"
        :d="`M ${RAIL_X} ${c.top + c.stubY} H ${RAIL_W}`"
        stroke="currentColor"
        stroke-width="1.5"
      />
    </svg>

    <div class="flex flex-col" :style="{ gap: `${ROW_GAP}px` }">
      <div v-for="c in nodes" :key="c.key" class="flex">
        <!-- 标签列:worktree = 横向连接线;branch = 占位 pill + 分线 -->
        <div class="shrink-0" :style="{ width: `${LABEL_W}px` }">
          <svg
            v-if="c.kind === 'worktree'"
            :width="LABEL_W"
            :height="ROW_H"
            class="text-border"
            fill="none"
            aria-hidden="true"
          >
            <path
              :d="`M 0 ${ROW_H / 2} H ${LABEL_W - 8}`"
              stroke="currentColor"
              stroke-width="1.5"
            />
            <path :d="arrowhead(LABEL_W, ROW_H / 2)" fill="currentColor" stroke="none" />
          </svg>
          <!-- 分支 pill 与右侧子 worktree 栈顶部对齐(不垂直居中) -->
          <div v-else class="flex h-full items-start">
            <div
              class="flex shrink-0 items-center"
              :style="{ width: `${PILL_W}px`, height: `${ROW_H}px` }"
            >
              <div
                class="flex min-w-0 flex-1 items-center gap-1.5 rounded-md border border-dashed px-2.5 py-1.5 text-xs text-muted-foreground"
                :title="t('git.worktree.branchBase', { name: c.label })"
              >
                <GitBranch class="h-3.5 w-3.5 shrink-0" />
                <span class="truncate font-mono">{{ c.label }}</span>
              </div>
            </div>
            <svg
              :width="LANE_W"
              :height="c.totalH"
              class="shrink-0 text-border"
              fill="none"
              aria-hidden="true"
            >
              <template v-for="gc in c.children" :key="gc.key">
                <path :d="laneConnector(gc)" stroke="currentColor" stroke-width="1.5" />
                <path
                  :d="arrowhead(LANE_W, gc.top + ROW_H / 2)"
                  fill="currentColor"
                  stroke="none"
                />
              </template>
            </svg>
          </div>
        </div>

        <!-- 内容列:worktree = 卡片(+ 子导轨区);branch = 右侧竖向堆叠的子 worktree -->
        <div class="min-w-0 flex-1">
          <template v-if="c.kind === 'worktree'">
            <WorktreeGraphCard
              :node="c"
              :active-path="activePath"
              :rebasing-path="rebasingPath"
              :rebase-interrupted-path="rebaseInterruptedPath"
              :aborting="aborting"
              @merge="emit('merge', $event)"
              @rebase="emit('rebase', $event)"
              @abort-rebase="emit('abortRebase', $event)"
              @remove="emit('remove', $event)"
            />
            <WorktreeGraphRail
              v-if="c.children.length"
              :nodes="c.children"
              :active-path="activePath"
              :rebasing-path="rebasingPath"
              :rebase-interrupted-path="rebaseInterruptedPath"
              :aborting="aborting"
              @merge="emit('merge', $event)"
              @rebase="emit('rebase', $event)"
              @abort-rebase="emit('abortRebase', $event)"
              @remove="emit('remove', $event)"
            />
          </template>
          <div v-else class="flex flex-col" :style="{ gap: `${ROW_GAP}px` }">
            <div v-for="gc in c.children" :key="gc.key">
              <WorktreeGraphCard
                :node="gc"
                :active-path="activePath"
                :rebasing-path="rebasingPath"
                :rebase-interrupted-path="rebaseInterruptedPath"
                :aborting="aborting"
                @merge="emit('merge', $event)"
                @rebase="emit('rebase', $event)"
                @abort-rebase="emit('abortRebase', $event)"
                @remove="emit('remove', $event)"
              />
              <WorktreeGraphRail
                v-if="gc.children.length"
                :nodes="gc.children"
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
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
