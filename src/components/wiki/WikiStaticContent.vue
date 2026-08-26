<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { BookOpenText, FileCode, LoaderCircle, RefreshCw } from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { parseWikiSources } from "@/lib/wiki-parse";
import type { SupportedLocale } from "@/i18n";
import type { WikiPageData } from "@/types";
import SourceFileDialog from "./SourceFileDialog.vue";

interface SourceTarget {
  path: string;
  start: number | null;
  end: number | null;
}

const props = defineProps<{
  page: WikiPageData;
  pages: WikiPageData[];
  projectRoot: string;
  language: SupportedLocale;
  regenerating: boolean;
}>();

const emit = defineEmits<{
  regenerate: [page: WikiPageData];
  selectRelated: [id: string];
}>();

const { t } = useI18n();
const sourceTarget = ref<SourceTarget | null>(null);
const parsed = computed(() => parseWikiSources(props.page.content, props.page.relevantFiles));
const relatedPages = computed<WikiPageData[]>(() => {
  const seen = new Set<string>();
  const related: WikiPageData[] = [];
  for (const id of props.page.relatedPages) {
    if (id === props.page.id || seen.has(id)) {
      continue;
    }
    const target = props.pages.find((item) => item.id === id);
    if (target) {
      seen.add(id);
      related.push(target);
    }
  }
  return related;
});

const controls: ControlsConfig = {
  table: {
    copy: true,
    download: true,
    fullscreen: true,
    customize: createTableCustomize(t),
  },
  code: { copy: true, collapse: true },
};
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;
const beforeDownload = createBeforeDownload(t);

function openSource(path: string) {
  const range = parsed.value.ranges.get(path);
  sourceTarget.value = {
    path,
    start: range?.start ?? null,
    end: range?.end ?? null,
  };
}

function sourceRangeLabel(path: string): string {
  const range = parsed.value.ranges.get(path);
  if (!range) {
    return "";
  }
  return range.end > range.start ? `:${range.start}-${range.end}` : `:${range.start}`;
}
</script>

<template>
  <div class="mb-3 flex items-center gap-2">
    <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
      {{ page.file }}
    </span>
    <Button
      variant="ghost"
      size="sm"
      :disabled="regenerating"
      :title="t('wiki.regeneratePage')"
      @click="emit('regenerate', page)"
    >
      <LoaderCircle v-if="regenerating" class="h-3.5 w-3.5 animate-spin" />
      <RefreshCw v-else class="h-3.5 w-3.5" />
    </Button>
  </div>

  <div v-if="page.content">
    <Markdown
      mode="static"
      :content="parsed.body"
      :controls="controls"
      :theme-element="themeElement"
      :locale="language"
      :before-download="beforeDownload"
    />

    <div v-if="page.relevantFiles.length" class="mt-6 border-t pt-3">
      <p class="mb-2 text-xs font-medium text-muted-foreground">{{ t("wiki.sources") }}</p>
      <div class="flex flex-wrap gap-1.5">
        <button
          v-for="file in page.relevantFiles"
          :key="file"
          type="button"
          class="flex max-w-full items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
          :title="file"
          @click="openSource(file)"
        >
          <FileCode class="h-3 w-3 shrink-0" />
          <span class="min-w-0 truncate [direction:rtl] [text-align:left]">{{ file }}</span>
          <span v-if="sourceRangeLabel(file)" class="shrink-0 text-primary">
            {{ sourceRangeLabel(file) }}
          </span>
        </button>
      </div>
    </div>

    <div v-if="relatedPages.length" class="mt-6 border-t pt-3">
      <p class="mb-2 text-xs font-medium text-muted-foreground">
        {{ t("wiki.relatedPages") }}
      </p>
      <div class="flex flex-wrap gap-1.5">
        <button
          v-for="related in relatedPages"
          :key="related.id"
          type="button"
          class="flex max-w-full items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          :title="related.title"
          @click="emit('selectRelated', related.id)"
        >
          <BookOpenText class="h-3 w-3 shrink-0" />
          <span class="truncate">{{ related.title }}</span>
        </button>
      </div>
    </div>
  </div>

  <div
    v-else
    class="flex flex-col items-center gap-2 rounded-md border border-dashed py-12 text-sm text-muted-foreground"
  >
    <p>{{ t("wiki.emptyContent") }}</p>
    <Button variant="outline" size="sm" @click="emit('regenerate', page)">
      <RefreshCw class="h-4 w-4" />
      {{ t("wiki.regeneratePage") }}
    </Button>
  </div>

  <SourceFileDialog
    :root="projectRoot"
    :rel-path="sourceTarget?.path ?? null"
    :start-line="sourceTarget?.start ?? null"
    :end-line="sourceTarget?.end ?? null"
    @close="sourceTarget = null"
  />
</template>
