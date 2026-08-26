<script setup lang="ts">
import { nextTick, ref } from "vue";
import { useI18n } from "vue-i18n";
import { FolderTree, Search } from "@lucide/vue";
import FileTreeList from "@/components/common/FileTreeList.vue";
import TextSearchPanel from "@/components/files/TextSearchPanel.vue";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { FileTreeRow } from "@/lib/file-tree";
import type { FindQuery } from "@/lib/text-search";
import type { ProjectFileEntry } from "@/types";

defineProps<{
  empty: boolean;
  error: boolean;
  loading: boolean;
  root: string;
  rows: FileTreeRow<ProjectFileEntry>[];
  selected: string | null;
}>();

const emit = defineEmits<{
  open: [path: string, line: number, query: FindQuery];
  select: [path: string];
  toggle: [path: string];
}>();

const view = defineModel<"tree" | "search">("view", { required: true });
const { t } = useI18n();
const searchPanel = ref<InstanceType<typeof TextSearchPanel> | null>(null);

async function focusSearch() {
  await nextTick();
  searchPanel.value?.focusInput();
}

defineExpose({ focusSearch });
</script>

<template>
  <div class="flex shrink-0 items-center gap-1.5 border-b px-2 py-2">
    <span class="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
      {{ view === "tree" ? t("files.treeView") : t("files.textSearchTitle") }}
    </span>
    <div class="flex shrink-0 items-center gap-0.5">
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :class="view === 'tree' ? 'bg-accent' : ''"
        :title="t('files.treeView')"
        @click="view = 'tree'"
      >
        <FolderTree class="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :class="view === 'search' ? 'bg-accent' : ''"
        :title="t('files.searchView')"
        @click="view = 'search'"
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
    v-else
    ref="searchPanel"
    :root="root"
    @open="(...args) => emit('open', ...args)"
  />
</template>
