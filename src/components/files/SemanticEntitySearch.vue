<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2, Search } from "@lucide/vue";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useSemanticRequest } from "@/composables/useSemanticRequest";
import { cmd } from "@/lib/tauri";
import { debounce } from "@/lib/utils";
import type { SemanticEntityRef, SemanticFindResult } from "@/types";

// 全项目实体搜索:至少 2 字符、250ms 防抖;结果点击由父组件打开文件并定位。
// 仅在用户显式进入本面板时工作,不与全文搜索混用。

const props = defineProps<{ root: string }>();

const emit = defineEmits<{
  open: [filePath: string, startLine: number, endLine: number];
}>();

const { t } = useI18n();

const query = ref("");

const request = useSemanticRequest((requestId: string, q: string) =>
  cmd<SemanticFindResult>("semantic_find_entities", { path: props.root, query: q, requestId }),
);

const debouncedSearch = debounce(() => {
  const q = query.value.trim();
  if (q.length < 2) {
    request.reset();
    return;
  }
  void request.run(q);
}, 250);

watch(query, () => debouncedSearch());
watch(
  () => props.root,
  () => request.reset(),
);
onBeforeUnmount(() => {
  debouncedSearch.cancel();
  request.cancel();
});

function openResult(entity: SemanticEntityRef) {
  emit("open", entity.filePath, entity.startLine, entity.endLine);
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div class="shrink-0 border-b p-2">
      <div class="relative">
        <Search
          class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="query"
          :placeholder="t('files.semantic.searchPlaceholder')"
          class="h-8 pl-8 text-sm"
        />
      </div>
    </div>
    <ScrollArea class="min-h-0 flex-1">
      <div
        v-if="request.loading.value"
        class="flex items-center gap-1.5 p-3 text-xs text-muted-foreground"
      >
        <Loader2 class="h-3 w-3 animate-spin" />
        {{ t("common.loading") }}
      </div>
      <p v-else-if="request.error.value" class="p-3 text-xs text-destructive">
        {{ request.error.value }}
      </p>
      <p v-else-if="query.trim().length < 2" class="p-3 text-xs text-muted-foreground">
        {{ t("files.semantic.searchMinChars") }}
      </p>
      <p
        v-else-if="!request.result.value?.results.length"
        class="p-3 text-xs text-muted-foreground"
      >
        {{ t("files.semantic.noResults") }}
      </p>
      <div v-else class="py-1">
        <p
          v-if="request.result.value.truncated"
          class="border-b px-3 py-1.5 text-xs text-muted-foreground"
        >
          {{ t("files.semantic.truncated") }}
        </p>
        <button
          v-for="entity in request.result.value.results"
          :key="entity.entityId ?? `${entity.filePath}:${entity.name}:${entity.startLine}`"
          type="button"
          class="flex w-full items-center gap-1.5 px-3 py-1 text-left hover:bg-accent/60"
          :title="entity.entityId ?? entity.filePath"
          @click="openResult(entity)"
        >
          <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ entity.name }}</span>
          <span class="shrink-0 text-[10px] text-muted-foreground">{{ entity.entityType }}</span>
          <span class="shrink-0 max-w-24 truncate text-[10px] text-muted-foreground">
            {{ entity.filePath }}:{{ entity.startLine }}
          </span>
        </button>
      </div>
    </ScrollArea>
  </div>
</template>
