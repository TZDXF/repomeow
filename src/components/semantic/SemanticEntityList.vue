<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { entityDisplayName } from "@/lib/semantic";
import type { SemanticEntityRef } from "@/types";

// 语义实体列表:影响分析各页签共用;showDepth 时展示传递深度徽标(受影响页签)。

withDefaults(
  defineProps<{
    items: (SemanticEntityRef & { depth?: number })[];
    showDepth?: boolean;
    emptyText: string;
  }>(),
  { showDepth: false },
);

const emit = defineEmits<{
  open: [entity: SemanticEntityRef];
}>();

const { t } = useI18n();

function key(entity: SemanticEntityRef): string {
  return entity.entityId ?? `${entity.filePath}:${entity.name}:${entity.startLine}`;
}
</script>

<template>
  <p v-if="!items.length" class="px-3 py-4 text-xs text-muted-foreground">{{ emptyText }}</p>
  <div v-else class="py-1">
    <button
      v-for="entity in items"
      :key="key(entity)"
      type="button"
      class="flex w-full items-center gap-2 px-3 py-1 text-left hover:bg-accent/60"
      :title="entity.entityId ?? entity.filePath"
      @click="emit('open', entity)"
    >
      <span
        v-if="showDepth && entity.depth != null"
        class="shrink-0 rounded bg-muted px-1 text-[10px] text-muted-foreground"
      >
        {{ t("files.semantic.depthBadge", { depth: entity.depth }) }}
      </span>
      <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ entityDisplayName(entity) }}</span>
      <span class="shrink-0 text-[10px] text-muted-foreground">{{ entity.entityType }}</span>
      <span class="shrink-0 max-w-40 truncate text-[10px] text-muted-foreground">
        {{ entity.filePath }}:{{ entity.startLine }}
      </span>
    </button>
  </div>
</template>
