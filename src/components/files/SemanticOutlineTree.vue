<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronDown, ChevronRight, History, Network } from "@lucide/vue";
import {
  COLLAPSED_ENTITY_TYPES,
  entityDisplayName,
  type SemanticOutlineNode,
} from "@/lib/semantic";
import type { SemanticBlameEntry, SemanticFileEntity } from "@/types";

// 结构树递归节点:高密度类型(property/import/section)按类型聚合成折叠行,
// 其余实体按文档序展示,子级缩进。点击行由父级定位到源码。

const props = defineProps<{
  nodes: SemanticOutlineNode[];
  depth: number;
  selectedId: string | null;
  /** 负责人/最后修改(blame)标注,key 为 `${name}:${startLine}` */
  blame?: Map<string, SemanticBlameEntry>;
}>();

const emit = defineEmits<{
  select: [entity: SemanticFileEntity];
  impact: [entity: SemanticFileEntity];
  history: [entity: SemanticFileEntity];
}>();

const { t } = useI18n();

/** blame 标注按 名称+起始行 匹配(实体行区间与 blame 行区间同源) */
function blameOf(node: SemanticOutlineNode): SemanticBlameEntry | undefined {
  return props.blame?.get(`${node.entity.name}:${node.entity.startLine}`);
}

const visible = computed(() =>
  props.nodes.filter((node) => !COLLAPSED_ENTITY_TYPES.has(node.entity.entityType)),
);

/** 高密度实体按类型聚合 */
const collapsedGroups = computed(() => {
  const groups = new Map<string, SemanticOutlineNode[]>();
  for (const node of props.nodes) {
    if (COLLAPSED_ENTITY_TYPES.has(node.entity.entityType)) {
      const list = groups.get(node.entity.entityType);
      if (list) list.push(node);
      else groups.set(node.entity.entityType, [node]);
    }
  }
  return [...groups.entries()];
});

const expandedGroups = ref(new Set<string>());
const collapsedNodes = ref(new Set<string>());

function toggleGroup(type: string) {
  const next = new Set(expandedGroups.value);
  if (next.has(type)) next.delete(type);
  else next.add(type);
  expandedGroups.value = next;
}

function toggleNode(id: string) {
  const next = new Set(collapsedNodes.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  collapsedNodes.value = next;
}

function nodeKey(node: SemanticOutlineNode): string {
  return node.entity.entityId ?? `${node.entity.name}:${node.entity.startLine}`;
}
</script>

<template>
  <div>
    <template v-for="node in visible" :key="nodeKey(node)">
      <div
        class="flex w-full items-center gap-1 py-0.5 pr-2 text-left hover:bg-accent/60"
        :class="selectedId && selectedId === node.entity.entityId ? 'bg-accent' : ''"
        :style="{ paddingLeft: `${8 + depth * 12}px` }"
      >
        <button
          v-if="node.children.length"
          type="button"
          class="flex h-3.5 w-3.5 shrink-0 items-center justify-center text-muted-foreground"
          @click.stop="toggleNode(nodeKey(node))"
        >
          <component
            :is="collapsedNodes.has(nodeKey(node)) ? ChevronRight : ChevronDown"
            class="h-3 w-3"
          />
        </button>
        <span v-else class="w-3.5 shrink-0" />
        <button
          type="button"
          class="group flex min-w-0 flex-1 items-center gap-1.5 text-left"
          :title="`${node.entity.entityType} · L${node.entity.startLine}-${node.entity.endLine}`"
          @click="emit('select', node.entity)"
        >
          <span class="min-w-0 flex-1 truncate font-mono text-xs">
            {{ entityDisplayName(node.entity) }}
          </span>
          <span
            v-if="blameOf(node)"
            class="shrink-0 max-w-28 truncate text-[10px] text-muted-foreground"
            :title="blameOf(node)?.summary"
          >
            {{ blameOf(node)?.author }} · {{ blameOf(node)?.date }}
          </span>
          <span
            class="hidden h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground group-hover:flex"
            :title="t('files.semantic.historyAction')"
            @click.stop="emit('history', node.entity)"
          >
            <History class="h-3 w-3" />
          </span>
          <span
            class="hidden h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground group-hover:flex"
            :title="t('files.semantic.impactAction')"
            @click.stop="emit('impact', node.entity)"
          >
            <Network class="h-3 w-3" />
          </span>
          <span class="shrink-0 text-[10px] text-muted-foreground">{{
            node.entity.entityType
          }}</span>
          <span class="shrink-0 text-[10px] text-muted-foreground"
            >:{{ node.entity.startLine }}</span
          >
        </button>
      </div>
      <SemanticOutlineTree
        v-if="node.children.length && !collapsedNodes.has(nodeKey(node))"
        :nodes="node.children"
        :depth="depth + 1"
        :selected-id="selectedId"
        :blame="blame"
        @select="(entity) => emit('select', entity)"
        @impact="(entity) => emit('impact', entity)"
        @history="(entity) => emit('history', entity)"
      />
    </template>

    <!-- 高密度类型聚合行 -->
    <template v-for="[type, group] in collapsedGroups" :key="`group-${type}`">
      <button
        type="button"
        class="flex w-full items-center gap-1 py-0.5 pr-2 text-left text-muted-foreground hover:bg-accent/60"
        :style="{ paddingLeft: `${8 + depth * 12}px` }"
        @click="toggleGroup(`${depth}:${type}`)"
      >
        <component
          :is="expandedGroups.has(`${depth}:${type}`) ? ChevronDown : ChevronRight"
          class="h-3 w-3 shrink-0"
        />
        <span class="text-[11px]">
          {{ t("files.semantic.collapsedGroup", { type, count: group.length }) }}
        </span>
      </button>
      <SemanticOutlineTree
        v-if="expandedGroups.has(`${depth}:${type}`)"
        :nodes="group"
        :depth="depth + 1"
        :selected-id="selectedId"
        :blame="blame"
        @select="(entity) => emit('select', entity)"
        @impact="(entity) => emit('impact', entity)"
        @history="(entity) => emit('history', entity)"
      />
    </template>
  </div>
</template>
