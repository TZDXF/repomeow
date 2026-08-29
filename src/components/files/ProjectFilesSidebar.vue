<script setup lang="ts">
import { nextTick, ref } from "vue";
import { useI18n } from "vue-i18n";
import { FolderTree, ListTree, Search } from "@lucide/vue";
import FileTreeList from "@/components/common/FileTreeList.vue";
import TextSearchPanel from "@/components/files/TextSearchPanel.vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { FileTreeRow } from "@/lib/file-tree";
import type { FindQuery } from "@/lib/text-search";
import type { ProjectFileEntry } from "@/types";

const props = defineProps<{
  empty: boolean;
  error: boolean;
  loading: boolean;
  root: string;
  rows: FileTreeRow<ProjectFileEntry>[];
  selected: string | null;
  /** 当前文件是可预览文本时启用「结构」视图 */
  outlineEnabled: boolean;
}>();

const emit = defineEmits<{
  open: [path: string, line: number, query: FindQuery];
  select: [path: string];
  toggle: [path: string];
}>();

const view = defineModel<"tree" | "search" | "outline">("view", { required: true });
const { t } = useI18n();
const searchPanel = ref<InstanceType<typeof TextSearchPanel> | null>(null);

async function focusSearch() {
  await nextTick();
  searchPanel.value?.focusInput();
}

defineExpose({ focusSearch });

const title = () =>
  view.value === "tree"
    ? t("files.treeView")
    : view.value === "search"
      ? t("files.textSearchTitle")
      : t("files.semantic.outlineTitle");

// 当前文件不可解析时切 outline 无意义,回退树视图
function setView(next: "tree" | "search" | "outline") {
  if (next === "outline" && !props.outlineEnabled) return;
  view.value = next;
}
</script>

<template>
  <div class="flex shrink-0 items-center gap-1.5 border-b px-2 py-2">
    <span class="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
      {{ title() }}
    </span>
    <div class="flex shrink-0 items-center gap-0.5">
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :class="view === 'tree' ? 'bg-accent' : ''"
        :title="t('files.treeView')"
        @click="setView('tree')"
      >
        <FolderTree class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :class="view === 'outline' ? 'bg-accent' : ''"
        :disabled="!outlineEnabled"
        :title="t('files.semantic.outlineTitle')"
        @click="setView('outline')"
      >
        <ListTree class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :class="view === 'search' ? 'bg-accent' : ''"
        :title="t('files.searchView')"
        @click="setView('search')"
      >
        <Search class="h-3.5 w-3.5" />
      </Button>
    </div>
  </div>
  <template v-if="view === 'tree'">
    <ScrollArea class="min-h-0 flex-1">
      <p v-if="loading" class="p-4 text-sm text-muted-foreground">
        {{ t("common.loading") }}
      </p>
      <p v-else-if="error" class="p-4 text-sm text-destructive">
        {{ t("files.listFailed") }}
      </p>
      <p v-else-if="empty" class="p-4 text-sm text-muted-foreground">
        {{ t("files.empty") }}
      </p>
      <FileTreeList
        v-else
        :rows="rows"
        :selected="selected"
        @select="(row) => emit('select', row.fullPath)"
        @toggle="(row) => emit('toggle', row.fullPath)"
      />
    </ScrollArea>
  </template>
  <TextSearchPanel
    v-else-if="view === 'search'"
    ref="searchPanel"
    :root="root"
    @open="(...args) => emit('open', ...args)"
  />
  <!-- 结构视图由父组件经插槽提供(需要文件预览与定位上下文) -->
  <div v-else class="min-h-0 flex-1">
    <slot name="outline" />
  </div>
</template>
