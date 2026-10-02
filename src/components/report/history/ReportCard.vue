<script setup lang="ts">
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronRight, Download, Trash2 } from "@lucide/vue";
import { Markdown, type ControlsConfig } from "vue-stream-markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import ScrollArea from "@/components/common/ScrollArea.vue";
import { formatCommitTime, formatLocalDateTime } from "@/lib/format";
import type { createBeforeDownload } from "@/lib/markdown-download";
import type { ReportHistoryDetail } from "@/types";

const { t } = useI18n();
defineProps<{
  report: ReportHistoryDetail;
  /** 手风琴展开态(父级保证同时只展开一条) */
  open: boolean;
  exporting: boolean;
  controls: ControlsConfig;
  themeElement: () => HTMLElement;
  locale: string;
  beforeDownload: ReturnType<typeof createBeforeDownload>;
}>();
const emit = defineEmits<{
  "update:open": [open: boolean];
  export: [report: ReportHistoryDetail];
  delete: [id: number];
}>();

/** 报告内各项目提交明细的展开态(按项目名记录,卡片实例随报告 id 复用) */
const commitOpen = ref<Record<string, boolean>>({});
</script>

<template>
  <Collapsible :open="open" class="rounded-lg border" @update:open="emit('update:open', $event)">
    <CollapsibleTrigger
      class="group flex w-full cursor-pointer items-center gap-2 rounded-t-lg px-3 py-2.5 text-left hover:bg-accent/50"
      :class="!open && 'rounded-b-lg'"
    >
      <ChevronRight
        class="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform"
        :class="{ 'rotate-90': open }"
      />
      <span class="min-w-0 flex-1 truncate text-xs">
        <span class="font-medium"
          >{{ report.projectNames.slice(0, 3).join(", ")
          }}{{ report.projectNames.length > 3 ? ` +${report.projectNames.length - 3}` : "" }}</span
        >
        <span
          class="ml-2 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100"
        >
          {{ formatLocalDateTime(report.createdAt) }}
        </span>
      </span>
      <Badge
        variant="outline"
        class="shrink-0 text-[11px]"
        :class="
          report.periodType === 'weekly'
            ? 'border-violet-500/40 bg-violet-500/10 text-violet-600 dark:text-violet-400'
            : ''
        "
      >
        {{
          t(report.periodType === "weekly" ? "reportHistory.typeWeekly" : "reportHistory.typeDaily")
        }}
      </Badge>
      <Badge variant="secondary" class="shrink-0 text-[11px]">
        {{ t("reportHistory.totalCommits", { count: report.totalCommits }) }}
      </Badge>
      <Button
        variant="ghost"
        size="icon"
        class="h-5 w-5 shrink-0 text-muted-foreground"
        :title="t('reportHistory.export')"
        :disabled="exporting"
        @click.stop="emit('export', report)"
      >
        <Download class="h-3 w-3" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-5 w-5 shrink-0 text-muted-foreground hover:text-destructive"
        :title="t('common.delete')"
        @click.stop="emit('delete', report.id)"
      >
        <Trash2 class="h-3 w-3" />
      </Button>
    </CollapsibleTrigger>
    <CollapsibleContent>
      <div class="border-t">
        <!-- Markdown -->
        <div class="px-4 py-3 text-sm">
          <Markdown
            mode="static"
            :content="report.result"
            :controls="controls"
            :theme-element="themeElement"
            :locale="locale"
            :before-download="beforeDownload"
          />
        </div>
        <!-- Commits within this report -->
        <div v-if="report.commits.length" class="border-t">
          <Collapsible
            v-for="c in report.commits"
            :key="c.projectName"
            v-slot="{ open: expanded }"
            :open="commitOpen[c.projectName]"
            @update:open="commitOpen[c.projectName] = $event"
          >
            <CollapsibleTrigger
              class="flex w-full cursor-pointer items-center gap-1.5 px-4 py-1.5 text-left text-xs hover:bg-accent/50"
            >
              <ChevronRight
                class="h-3 w-3 shrink-0 text-muted-foreground transition-transform"
                :class="{ 'rotate-90': expanded }"
              />
              <span class="min-w-0 flex-1 truncate font-medium">{{ c.projectName }}</span>
              <span class="shrink-0 text-muted-foreground">{{ c.commits.length }}</span>
            </CollapsibleTrigger>
            <CollapsibleContent>
              <ScrollArea class="ml-5 max-h-40 border-l">
                <div
                  v-for="commit in c.commits"
                  :key="commit.hash + commit.date"
                  class="flex min-w-0 items-center gap-1.5 border-b px-2 py-0.5 text-[11px]"
                >
                  <code class="shrink-0 rounded bg-muted px-1 py-px font-mono text-[10px]">
                    {{ commit.hash }}
                  </code>
                  <span class="min-w-0 flex-1 truncate" :title="commit.subject">
                    {{ commit.subject }}
                  </span>
                  <span class="shrink-0 text-muted-foreground">
                    {{ formatCommitTime(commit.date) }}
                  </span>
                </div>
              </ScrollArea>
            </CollapsibleContent>
          </Collapsible>
        </div>
      </div>
    </CollapsibleContent>
  </Collapsible>
</template>
