<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { Search } from "@lucide/vue";
import { DropdownMenuCheckboxItem } from "@/components/ui/dropdown-menu";
import ScrollArea from "@/components/common/ScrollArea.vue";
import type { Tag } from "@/types";

const { t } = useI18n();
const props = defineProps<{ tags: Tag[]; checkedIds: number[] }>();
const emit = defineEmits<{ toggle: [tagId: number] }>();

const keyword = ref("");
const searchInput = ref<HTMLInputElement | null>(null);

const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  if (!kw) return props.tags;
  return props.tags.filter((t) => t.name.toLowerCase().includes(kw));
});

// 阻止字符键冒泡触发菜单的 type-ahead,Escape 保留给菜单关闭
function onSearchKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") e.stopPropagation();
}

onMounted(() => {
  nextTick(() => searchInput.value?.focus());
});
</script>

<template>
  <div class="px-1 pb-1">
    <div class="relative">
      <Search class="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
      <input
        ref="searchInput"
        v-model="keyword"
        :placeholder="t('tags.checkList.searchPlaceholder')"
        class="h-7 w-full rounded-md border border-input bg-transparent pl-7 pr-2 text-xs outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
        @keydown="onSearchKeydown"
      />
    </div>
  </div>
  <ScrollArea class="max-h-56">
    <DropdownMenuCheckboxItem
      v-for="tag in filtered"
      :key="tag.id"
      :model-value="checkedIds.includes(tag.id)"
      @update:model-value="emit('toggle', tag.id)"
      @select.prevent
    >
      <span class="mr-1 h-2.5 w-2.5 rounded-full" :style="{ backgroundColor: tag.color }" />
      {{ tag.name }}
    </DropdownMenuCheckboxItem>
    <p v-if="!tags.length" class="px-2 py-1.5 text-xs text-muted-foreground">
      {{ t("tags.checkList.empty") }}
    </p>
    <p v-else-if="!filtered.length" class="px-2 py-1.5 text-xs text-muted-foreground">
      {{ t("tags.checkList.noMatch") }}
    </p>
  </ScrollArea>
</template>
