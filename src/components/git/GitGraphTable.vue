<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { GitBranch, Tag as TagIcon } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { laneColor, type GraphEdgeLayout, type GraphNodeLayout } from "@/lib/git-graph";
import type { GitGraphColumnKey } from "@/composables/git/useGitGraphSizing";
import type { GitGraphCommit } from "@/types";

interface VisibleGraphNode {
  data: GraphNodeLayout;
  index: number;
}

interface ColumnWidths {
  graph: number;
  descDelta: number;
  author: number;
  commit: number;
  date: number;
}

const props = defineProps<{
  visibleNodes: VisibleGraphNode[];
  edges: GraphEdgeLayout[];
  startIndex: number;
  endIndex: number;
  totalCount: number;
  streamDone: boolean;
  selectedHash?: string;
  matchHashes: Set<string>;
  graphWidth: number;
  graphColWidth: number;
  graphClipPath: string;
  descColWidth: number;
  totalWidth: number;
  colWidths: ColumnWidths;
}>();

const emit = defineEmits<{
  select: [commit: GitGraphCommit];
  startColResize: [key: GitGraphColumnKey, event: PointerEvent];
  resetDescWidth: [];
}>();

const { t } = useI18n();

const ROW_HEIGHT = 32;
const LANE_WIDTH = 16;
const GRAPH_PADDING = 4;
const NODE_RADIUS = 4;
const HEADER_HEIGHT = 32;

const visibleEdges = computed(() => {
  const all = props.edges;
  let lo = 0;
  let hi = all.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (all[mid].fromRow <= props.endIndex) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  const result: GraphEdgeLayout[] = [];
  for (let i = 0; i < lo; i++) {
    if (all[i].toRow >= props.startIndex) {
      result.push(all[i]);
    }
  }
  return result;
});

const svgOffsetY = computed(() => props.startIndex * ROW_HEIGHT);

function nodeX(lane: number) {
  return GRAPH_PADDING + lane * LANE_WIDTH + LANE_WIDTH / 2;
}

function nodeYRelative(row: number) {
  return row * ROW_HEIGHT + ROW_HEIGHT / 2 - svgOffsetY.value;
}

function edgePath(edge: GraphEdgeLayout): string {
  const x1 = nodeX(edge.fromLane);
  const y1 = nodeYRelative(edge.fromRow);
  const x2 = nodeX(edge.toLane);
  const y2 = nodeYRelative(edge.toRow);
  if (x1 === x2) {
    return `M ${x1} ${y1} L ${x2} ${y2}`;
  }
  const bendY = Math.min(y1 + ROW_HEIGHT, y2);
  return `M ${x1} ${y1} C ${x1} ${y1 + ROW_HEIGHT * 0.6}, ${x2} ${y1 + ROW_HEIGHT * 0.4}, ${x2} ${bendY} L ${x2} ${y2}`;
}

function shortHash(hash: string) {
  return hash.slice(0, 7);
}

function isTag(refName: string) {
  return refName.startsWith("tag: ");
}

function tagName(refName: string) {
  return refName.slice(5);
}
</script>

<template>
  <!-- 表头:列名 + 拖拽分隔条调整列宽(图谱列可拖窄,图形裁剪而非压缩) -->
  <div
    class="sticky top-0 z-20 flex items-center border-b bg-background text-xs font-medium text-muted-foreground"
    :style="{ width: `${totalWidth}px`, height: `${HEADER_HEIGHT}px` }"
  >
    <div class="relative flex h-full items-center px-2" :style="{ width: `${graphColWidth}px` }">
      {{ t("git.graph.columns.graph") }}
      <span
        class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
        @pointerdown="emit('startColResize', 'graph', $event)"
      />
    </div>
    <div
      class="relative flex h-full items-center border-l px-2"
      :style="{ width: `${descColWidth}px` }"
    >
      {{ t("git.graph.columns.description") }}
      <span
        class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
        @pointerdown="emit('startColResize', 'desc', $event)"
        @dblclick="emit('resetDescWidth')"
      />
    </div>
    <div
      class="relative flex h-full items-center border-l px-2"
      :style="{ width: `${colWidths.author}px` }"
    >
      {{ t("git.graph.columns.author") }}
      <span
        class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
        @pointerdown="emit('startColResize', 'author', $event)"
      />
    </div>
    <div
      class="relative flex h-full items-center border-l px-2"
      :style="{ width: `${colWidths.commit}px` }"
    >
      {{ t("git.graph.columns.commit") }}
      <span
        class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
        @pointerdown="emit('startColResize', 'commit', $event)"
      />
    </div>
    <div
      class="relative flex h-full items-center border-l px-2"
      :style="{ width: `${colWidths.date}px` }"
    >
      {{ t("git.graph.columns.date") }}
      <span
        class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
        @pointerdown="emit('startColResize', 'date', $event)"
      />
    </div>
  </div>

  <div
    class="relative"
    :style="{ height: `${totalCount * ROW_HEIGHT}px`, width: `${totalWidth}px` }"
  >
    <!-- SVG 只覆盖可视窗口;图谱列变窄时仅裁剪,不压缩泳道 -->
    <svg
      class="pointer-events-none absolute left-0 z-10 overflow-visible"
      :style="{
        top: `${svgOffsetY}px`,
        width: `${graphWidth}px`,
        height: `${(endIndex - startIndex + 1) * ROW_HEIGHT}px`,
        clipPath: graphClipPath,
      }"
    >
      <path
        v-for="(edge, index) in visibleEdges"
        :key="index"
        :d="edgePath(edge)"
        :stroke="laneColor(edge.color)"
        stroke-width="1.5"
        fill="none"
      />
      <template v-for="{ data: node, index } in visibleNodes" :key="node.commit.hash">
        <circle
          v-if="node.commit.is_head"
          :cx="nodeX(node.lane)"
          :cy="nodeYRelative(index)"
          :r="NODE_RADIUS + 2.5"
          fill="none"
          :stroke="laneColor(node.color)"
          stroke-width="1.5"
        />
        <circle
          :cx="nodeX(node.lane)"
          :cy="nodeYRelative(index)"
          :r="selectedHash === node.commit.hash ? NODE_RADIUS + 1 : NODE_RADIUS"
          :fill="laneColor(node.color)"
          class="stroke-background"
          stroke-width="2"
        />
      </template>
    </svg>

    <div
      v-for="{ data: node, index } in visibleNodes"
      :key="node.commit.hash"
      class="absolute left-0 flex cursor-pointer items-center transition-colors hover:bg-accent/60"
      :class="
        selectedHash === node.commit.hash
          ? 'bg-accent'
          : matchHashes.has(node.commit.hash)
            ? 'bg-amber-500/15'
            : ''
      "
      :style="{
        top: `${index * ROW_HEIGHT}px`,
        height: `${ROW_HEIGHT}px`,
        width: `${totalWidth}px`,
      }"
      @click="emit('select', node.commit)"
    >
      <div class="h-full shrink-0" :style="{ width: `${graphColWidth}px` }" />
      <div
        class="flex h-full min-w-0 shrink-0 items-center gap-2 overflow-hidden px-2"
        :style="{ width: `${descColWidth}px` }"
      >
        <Badge v-if="node.commit.is_head" variant="default" class="h-5 shrink-0 px-1.5 text-[10px]">
          HEAD
        </Badge>
        <template v-for="refName in node.commit.refs" :key="refName">
          <Badge
            v-if="isTag(refName)"
            variant="outline"
            class="h-5 max-w-40 shrink-0 gap-1 px-1.5 text-[10px] text-amber-600 dark:text-amber-400"
          >
            <TagIcon class="h-2.5 w-2.5 shrink-0" />
            <span class="truncate">{{ tagName(refName) }}</span>
          </Badge>
          <Badge v-else variant="secondary" class="h-5 max-w-40 shrink-0 gap-1 px-1.5 text-[10px]">
            <GitBranch class="h-2.5 w-2.5 shrink-0" />
            <span class="truncate">{{ refName }}</span>
          </Badge>
        </template>
        <span class="min-w-0 flex-1 truncate text-sm">{{ node.commit.subject }}</span>
      </div>
      <span
        class="shrink-0 truncate px-2 text-xs text-muted-foreground"
        :style="{ width: `${colWidths.author}px` }"
      >
        {{ node.commit.author }}
      </span>
      <span
        class="shrink-0 truncate px-2 font-mono text-xs text-muted-foreground"
        :style="{ width: `${colWidths.commit}px` }"
      >
        {{ shortHash(node.commit.hash) }}
      </span>
      <span
        class="shrink-0 truncate px-2 text-xs text-muted-foreground"
        :style="{ width: `${colWidths.date}px` }"
      >
        {{ node.commit.date }}
      </span>
    </div>
  </div>

  <p
    v-if="!streamDone"
    class="sticky bottom-0 border-t bg-background/95 px-4 py-2 text-center text-xs text-muted-foreground backdrop-blur"
  >
    {{ t("git.graph.loadingMore", { count: totalCount }) }}
  </p>
</template>
