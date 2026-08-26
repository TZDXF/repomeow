<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { Copy } from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import type { SupportedLocale } from "@/i18n";

defineProps<{
  result: string;
  generating: boolean;
  savedHistoryId: number | null;
  language: SupportedLocale;
}>();

const emit = defineEmits<{ copy: [] }>();
const { t } = useI18n();
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
</script>

<template>
  <div class="flex min-h-0 min-w-0 flex-1 flex-col rounded-md border">
    <div class="flex shrink-0 items-center justify-between border-b px-3 py-1.5">
      <div class="flex items-center gap-1">
        <span class="text-xs text-muted-foreground">Markdown</span>
        <span
          v-if="savedHistoryId"
          class="rounded bg-green-100 px-1.5 py-px text-[10px] text-green-700 dark:bg-green-900/30 dark:text-green-400"
        >
          {{ t("report.saved") }}
        </span>
      </div>
      <Button
        v-if="result"
        variant="ghost"
        size="sm"
        class="h-6 gap-1 px-2 text-xs"
        @click="emit('copy')"
      >
        <Copy class="h-3 w-3" />
        {{ t("report.copy") }}
      </Button>
    </div>
    <ScrollArea class="min-h-0 flex-1">
      <p v-if="!result" class="p-4 text-sm text-muted-foreground">
        {{ generating ? t("report.generating") : "" }}
      </p>
      <div v-else class="p-4 text-sm">
        <Markdown
          mode="static"
          :content="result"
          :controls="controls"
          :theme-element="themeElement"
          :locale="language"
          :before-download="beforeDownload"
        />
      </div>
    </ScrollArea>
  </div>
</template>
