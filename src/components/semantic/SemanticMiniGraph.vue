<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import {
  dedupeEntityRefs,
  entityDisplayName,
  sliceGraphSide,
  truncateGraphLabel,
} from "@/lib/semantic";
import type { SemanticEntityRef } from "@/types";

// 影响分析小图(纯 SVG,无新依赖):仅渲染 target + 直接 dependencies/dependents
// ——即 sem impact 实际返回的边(调用者 → 目标 → 依赖),不伪造传递边;
// 传递影响在列表页签按 depth 展示。每侧最多 MAX_SIDE 个节点,超出折叠为 +N 占位(不连边)。

const props = defineProps<{
  target: SemanticEntityRef;
  dependencies: SemanticEntityRef[];
  dependents: SemanticEntityRef[];
}>();

const emit = defineEmits<{
  open: [entity: SemanticEntityRef];
}>();

const { t } = useI18n();

const MAX_SIDE = 8;
const NODE_W = 170;
const NODE_H = 26;
const GAP = 6;
const PAD_Y = 6;
const CAPTION_H = 16;
const COL_X = { dependents: 8, target: 234, dependencies: 460 } as const;
const WIDTH = COL_X.dependencies + NODE_W + 8;

interface GraphNode {
  x: number;
  y: number;
  label: string;
  tooltip: string;
  /** null = 「+N」占位节点(不可点、不连边) */
  entity: SemanticEntityRef | null;
}

const dependents = computed(() => dedupeEntityRefs(props.dependents));
const dependencies = computed(() => dedupeEntityRefs(props.dependencies));

const blockRows = computed(() => {
  const rows = (list: SemanticEntityRef[]) => {
    const { shown, extra } = sliceGraphSide(list, MAX_SIDE);
    return shown.length + (extra > 0 ? 1 : 0);
  };
  return Math.max(rows(dependents.value), rows(dependencies.value), 1);
});

const height = computed(
  () => CAPTION_H + PAD_Y * 2 + blockRows.value * NODE_H + (blockRows.value - 1) * GAP,
);

const targetY = computed(() => CAPTION_H + (height.value - CAPTION_H - NODE_H) / 2);

/** 一侧节点纵向居中排布,超出 MAX_SIDE 的部分折叠为一个 +N 占位节点 */
function placeSide(list: SemanticEntityRef[], x: number): GraphNode[] {
  const { shown, extra } = sliceGraphSide(list, MAX_SIDE);
  const rows = shown.length + (extra > 0 ? 1 : 0);
  if (!rows) return [];
  const blockH = rows * NODE_H + (rows - 1) * GAP;
  const y0 = CAPTION_H + (height.value - CAPTION_H - blockH) / 2;
  const nodes: GraphNode[] = shown.map((entity, i) => ({
    x,
    y: y0 + i * (NODE_H + GAP),
    label: truncateGraphLabel(entityDisplayName(entity)),
    tooltip: `${entity.entityType} · ${entity.filePath}:${entity.startLine}`,
    entity,
  }));
  if (extra > 0) {
    nodes.push({
      x,
      y: y0 + shown.length * (NODE_H + GAP),
      label: t("files.semantic.graphMore", { count: extra }),
      tooltip: t("files.semantic.graphMore", { count: extra }),
      entity: null,
    });
  }
  return nodes;
}

/** 左右两侧合并渲染(key 用坐标,实体与占位节点均稳定唯一) */
const sideNodes = computed(() => [
  ...placeSide(dependents.value, COL_X.dependents),
  ...placeSide(dependencies.value, COL_X.dependencies),
]);

const targetNode = computed<GraphNode>(() => ({
  x: COL_X.target,
  y: targetY.value,
  label: truncateGraphLabel(entityDisplayName(props.target)),
  tooltip: props.target.entityId ?? `${props.target.filePath}:${props.target.startLine}`,
  entity: props.target,
}));

function curve(from: readonly [number, number], to: readonly [number, number]): string {
  const mx = (from[0] + to[0]) / 2;
  return `M ${from[0]} ${from[1]} C ${mx} ${from[1]}, ${mx} ${to[1]}, ${to[0]} ${to[1]}`;
}

/** 边方向:调用者 → 目标(箭头入),目标 → 依赖(箭头出);占位节点不连边 */
const edges = computed(() => {
  const targetCy = targetY.value + NODE_H / 2;
  const out: string[] = [];
  for (const node of sideNodes.value) {
    if (!node.entity) continue;
    const cy = node.y + NODE_H / 2;
    out.push(
      node.x === COL_X.dependents
        ? curve([node.x + NODE_W, cy], [COL_X.target, targetCy])
        : curve([COL_X.target + NODE_W, targetCy], [node.x, cy]),
    );
  }
  return out;
});

function openNode(node: GraphNode) {
  if (node.entity) emit("open", node.entity);
}
</script>

<template>
  <svg :viewBox="`0 0 ${WIDTH} ${height}`" class="h-auto w-full select-none" role="img">
    <defs>
      <marker
        id="sem-mini-graph-arrow"
        viewBox="0 0 8 8"
        refX="7"
        refY="4"
        markerWidth="6"
        markerHeight="6"
        orient="auto"
      >
        <path d="M0,0 L8,4 L0,8 z" style="fill: var(--muted-foreground)" />
      </marker>
    </defs>

    <text :x="COL_X.dependents" y="11" class="fill-muted-foreground text-[10px]">
      {{ t("files.semantic.impactTab.dependents") }}
    </text>
    <text :x="COL_X.target" y="11" class="fill-muted-foreground text-[10px]">
      {{ t("files.semantic.graphTarget") }}
    </text>
    <text :x="COL_X.dependencies" y="11" class="fill-muted-foreground text-[10px]">
      {{ t("files.semantic.impactTab.dependencies") }}
    </text>

    <path
      v-for="(d, i) in edges"
      :key="i"
      :d="d"
      fill="none"
      stroke-width="1"
      class="stroke-muted-foreground/50"
      marker-end="url(#sem-mini-graph-arrow)"
    />

    <!-- 目标实体 -->
    <g
      class="cursor-pointer outline-none"
      tabindex="0"
      role="button"
      @click="openNode(targetNode)"
      @keydown.enter.prevent="openNode(targetNode)"
    >
      <title>{{ targetNode.tooltip }}</title>
      <rect
        :x="targetNode.x"
        :y="targetNode.y"
        :width="NODE_W"
        :height="NODE_H"
        rx="5"
        class="fill-primary"
      />
      <text
        :x="targetNode.x + 8"
        :y="targetNode.y + NODE_H / 2 + 3.5"
        class="fill-primary-foreground font-mono text-[11px]"
      >
        {{ targetNode.label }}
      </text>
    </g>

    <!-- 调用者(左)/ 依赖(右) -->
    <g
      v-for="node in sideNodes"
      :key="`${node.x}:${node.y}`"
      :class="node.entity ? 'cursor-pointer outline-none' : ''"
      :tabindex="node.entity ? 0 : undefined"
      :role="node.entity ? 'button' : undefined"
      @click="openNode(node)"
      @keydown.enter.prevent="openNode(node)"
    >
      <title>{{ node.tooltip }}</title>
      <rect
        :x="node.x"
        :y="node.y"
        :width="NODE_W"
        :height="NODE_H"
        rx="5"
        class="stroke-border transition-colors"
        :class="
          node.entity ? 'fill-card hover:fill-accent' : 'fill-transparent [stroke-dasharray:4_3]'
        "
      />
      <text
        :x="node.x + 8"
        :y="node.y + NODE_H / 2 + 3.5"
        class="font-mono text-[11px]"
        :class="node.entity ? 'fill-foreground' : 'fill-muted-foreground'"
      >
        {{ node.label }}
      </text>
    </g>
  </svg>
</template>
