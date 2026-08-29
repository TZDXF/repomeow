<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useSemanticRequest } from "@/composables/useSemanticRequest";
import { cmd } from "@/lib/tauri";
import { flattenRelationGroups } from "@/lib/semantic";
import type { SemanticEntityRef, SemanticRelationResult } from "@/types";

// 实体的「调用者 / 引用」关系面板:选中实体后懒加载,切实体/切页签才发请求。

const props = defineProps<{
  root: string;
  entity: SemanticEntityRef | null;
}>();

const emit = defineEmits<{
  open: [filePath: string, startLine: number, endLine: number];
}>();

const { t } = useI18n();

const tab = ref<"callers" | "refs">("callers");

const request = useSemanticRequest((requestId: string, kind: "callers" | "refs") => {
  const entity = props.entity;
  if (!entity) return Promise.reject(new Error("no entity"));
  return cmd<SemanticRelationResult>(`semantic_entity_${kind}`, {
    path: props.root,
    // entityId 优先;无可靠 id 时回退 名称+文件 消歧
    entityId: entity.entityId ?? undefined,
    entityName: entity.entityId ? undefined : entity.name,
    filePath: entity.filePath || undefined,
    requestId,
  });
});

const related = computed(() =>
  request.result.value ? flattenRelationGroups(request.result.value.groups) : [],
);

watch(
  [() => props.entity, tab],
  () => {
    if (props.entity) void request.run(tab.value);
    else request.reset();
  },
  { immediate: true },
);
</script>

<template>
  <div class="flex min-h-0 flex-col">
    <div class="flex shrink-0 items-center gap-1 px-2 py-1">
      <Button
        v-for="kind in ['callers', 'refs'] as const"
        :key="kind"
        variant="ghost"
        size="sm"
        class="h-6 px-2 text-[11px]"
        :class="tab === kind ? 'bg-accent' : 'text-muted-foreground'"
        @click="tab = kind"
      >
        {{ t(`files.semantic.${kind}`) }}
      </Button>
    </div>
    <ScrollArea class="max-h-48 min-h-0">
      <div
        v-if="request.loading.value"
        class="flex items-center gap-1.5 px-3 py-2 text-xs text-muted-foreground"
      >
        <Loader2 class="h-3 w-3 animate-spin" />
        {{ t("common.loading") }}
      </div>
      <p v-else-if="request.error.value" class="px-3 py-2 text-xs text-destructive">
        {{ request.error.value }}
      </p>
      <p v-else-if="!related.length" class="px-3 py-2 text-xs text-muted-foreground">
        {{ t("files.semantic.relationsEmpty") }}
      </p>
      <template v-else>
        <p
          v-if="request.result.value?.truncated"
          class="border-b px-3 py-1 text-[11px] text-muted-foreground"
        >
          {{ t("files.semantic.truncated") }}
        </p>
        <button
          v-for="item in related"
          :key="item.entityId ?? `${item.filePath}:${item.name}:${item.startLine}`"
          type="button"
          class="flex w-full items-center gap-1.5 px-3 py-1 text-left hover:bg-accent/60"
          :title="item.entityId ?? item.filePath"
          @click="emit('open', item.filePath, item.startLine, item.endLine)"
        >
          <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ item.name }}</span>
          <span class="shrink-0 text-[10px] text-muted-foreground">{{ item.entityType }}</span>
          <span class="shrink-0 max-w-24 truncate text-[10px] text-muted-foreground">
            {{ item.filePath }}:{{ item.startLine }}
          </span>
        </button>
      </template>
    </ScrollArea>
  </div>
</template>
