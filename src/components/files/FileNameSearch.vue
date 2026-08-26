<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Search } from "@lucide/vue";
import { Icon } from "@iconify/vue";
import { onClickOutside } from "@vueuse/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { fileIcon } from "@/lib/file-icons";
import { cmd } from "@/lib/tauri";
import { debounce } from "@/lib/utils";
import type { ProjectFileEntry } from "@/types";

const FILE_SEARCH_LIMIT = 50;

const props = defineProps<{
  root: string;
}>();

const emit = defineEmits<{
  open: [path: string];
}>();

const { t } = useI18n();
const open = ref(false);
const text = ref("");
const activeIndex = ref(0);
const results = ref<ProjectFileEntry[]>([]);
const limited = ref(false);
const box = ref<HTMLElement | null>(null);
const trigger = ref<InstanceType<typeof Button> | null>(null);
let searchSeq = 0;

const debouncedSearch = debounce((query: string) => void runSearch(query), 200);

watch(text, () => {
  activeIndex.value = 0;
  const query = text.value.trim();
  if (!query) {
    debouncedSearch.cancel();
    results.value = [];
    limited.value = false;
    return;
  }
  debouncedSearch(query);
});

async function runSearch(query: string) {
  if (!props.root) {
    return;
  }
  const seq = ++searchSeq;
  try {
    const entries = await cmd<ProjectFileEntry[]>("search_project_files", {
      path: props.root,
      query,
      limit: FILE_SEARCH_LIMIT + 1,
    });
    if (seq !== searchSeq) {
      return;
    }
    limited.value = entries.length > FILE_SEARCH_LIMIT;
    results.value = entries.slice(0, FILE_SEARCH_LIMIT);
  } catch {
    if (seq !== searchSeq) {
      return;
    }
    results.value = [];
    limited.value = false;
  }
}

function close() {
  open.value = false;
  text.value = "";
  activeIndex.value = 0;
  debouncedSearch.cancel();
  searchSeq++;
  results.value = [];
  limited.value = false;
}

function toggle() {
  if (open.value) {
    close();
    return;
  }
  open.value = true;
  void nextTick(() => box.value?.querySelector("input")?.focus());
}

function openResult(path: string) {
  emit("open", path);
  close();
}

async function onKeydown(event: KeyboardEvent) {
  const total = results.value.length;
  if (event.key === "ArrowDown") {
    event.preventDefault();
    if (total) {
      activeIndex.value = (activeIndex.value + 1) % total;
    }
  } else if (event.key === "ArrowUp") {
    event.preventDefault();
    if (total) {
      activeIndex.value = (activeIndex.value - 1 + total) % total;
    }
  } else if (event.key === "Enter") {
    event.preventDefault();
    debouncedSearch.cancel();
    const query = text.value.trim();
    if (query) {
      await runSearch(query);
    }
    const hit = results.value[activeIndex.value] ?? results.value[0];
    if (hit) {
      openResult(hit.path);
    }
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    close();
  }
}

onClickOutside(box, close, { ignore: [trigger] });
</script>

<template>
  <div
    v-if="open"
    ref="box"
    class="absolute left-1/2 top-1/2 z-50 w-[min(28rem,60%)] -translate-x-1/2 -translate-y-1/2"
  >
    <Search
      class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
    />
    <Input
      v-model="text"
      :placeholder="t('files.searchPlaceholder')"
      class="h-8 bg-background pl-8 text-sm"
      @keydown="onKeydown"
    />
    <div
      v-if="text.trim()"
      class="absolute left-0 right-0 top-full mt-1 max-h-80 overflow-auto rounded-md border bg-popover p-1 shadow-md"
    >
      <p v-if="!results.length" class="px-2 py-3 text-center text-xs text-muted-foreground">
        {{ t("files.noMatch") }}
      </p>
      <template v-else>
        <button
          v-for="(file, index) in results"
          :key="file.path"
          type="button"
          class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm"
          :class="index === activeIndex ? 'bg-accent text-accent-foreground' : ''"
          :title="file.path"
          @mouseenter="activeIndex = index"
          @click="openResult(file.path)"
        >
          <Icon
            :icon="fileIcon(file.path.slice(file.path.lastIndexOf('/') + 1))"
            class="h-4 w-4 shrink-0"
          />
          <span class="min-w-0 truncate">{{ file.path }}</span>
        </button>
        <p v-if="limited" class="border-t px-2 py-1.5 text-center text-xs text-muted-foreground">
          {{ t("files.searchLimited", { count: FILE_SEARCH_LIMIT }) }}
        </p>
      </template>
    </div>
  </div>
  <Button
    ref="trigger"
    variant="ghost"
    size="icon"
    class="h-8 w-8 shrink-0"
    :class="open ? 'bg-accent' : ''"
    :title="t('files.searchPlaceholder')"
    @click="toggle"
  >
    <Search class="h-4 w-4" />
  </Button>
</template>
