<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Clock, Loader2, Pencil, Play, Power, PowerOff, Tags, Trash2 } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatLocalDateTime } from "@/lib/format";
import {
  projectNames,
  tagNames,
  weekdayModeOf,
  weekdayShortNames,
  weeklyRangeLabel,
  type WeekdayMode,
} from "@/composables/report-schedule/schedule-utils";
import { useProjectsStore } from "@/stores/projects";
import { useTagsStore } from "@/stores/tags";
import type { ReportSchedule } from "@/types";

/** 报告定时任务列表:展示配置摘要,发出启停/编辑/删除/手动执行事件(不直接改数据) */
defineProps<{ schedules: ReportSchedule[]; runningIds: string[] }>();
const emit = defineEmits<{
  toggle: [schedule: ReportSchedule];
  edit: [schedule: ReportSchedule];
  remove: [id: string];
  runNow: [schedule: ReportSchedule];
}>();

const { t, locale } = useI18n();
const projectStore = useProjectsStore();
const tagsStore = useTagsStore();

const weekdayNames = computed(() => weekdayShortNames(locale.value));
const activeProjects = computed(() => projectStore.projects.filter((p) => !p.archived_at));

function weekdayLabel(mode: WeekdayMode) {
  if (mode === "chineseWorkday") return t("reportSchedule.chineseWorkdayOnly");
  if (mode === "weekdays") return t("reportSchedule.weekdaysOnly");
  return t("reportSchedule.everyday");
}

/** 周报周期描述:工作周模式显示固定文案;自定义模式显示 "周一 ~ 周五" */
function weeklyLabel(s: ReportSchedule) {
  if (s.weeklyWorkweek) return t("reportSchedule.workweekLabel");
  return weeklyRangeLabel(s, weekdayNames.value);
}

function scheduleProjectNames(ids: number[]) {
  return projectNames(ids, activeProjects.value);
}

function scheduleTagNames(ids: number[]) {
  return tagNames(ids, tagsStore.tags);
}

function lastRun(ts: number | null) {
  if (!ts) return t("reportSchedule.never");
  return formatLocalDateTime(ts);
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div v-for="s in schedules" :key="s.id" class="flex items-center gap-3 rounded-md border p-3">
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="truncate text-sm font-medium">{{
            s.name || t("reportSchedule.title")
          }}</span>
          <Badge variant="outline" class="text-[11px]">
            {{
              t(
                s.reportType === "weekly"
                  ? "reportSchedule.typeWeekly"
                  : "reportSchedule.typeDaily",
              )
            }}
          </Badge>
          <Badge :variant="s.enabled ? 'default' : 'secondary'" class="text-[11px]">
            {{ s.enabled ? t("reportSchedule.enabled") : t("reportSchedule.disabled") }}
          </Badge>
        </div>
        <div
          class="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-muted-foreground"
        >
          <span class="flex items-center gap-1">
            <Clock class="h-3 w-3" />
            {{ s.timeOfDay }}
          </span>
          <span v-if="s.reportType === 'weekly'">{{ weeklyLabel(s) }}</span>
          <span v-else
            >{{
              s.previousDay
                ? t("reportSchedule.dailyRangePrevious")
                : t("reportSchedule.dailyRangeToday")
            }}
            · {{ weekdayLabel(weekdayModeOf(s)) }}</span
          >
          <span
            >{{ t("reportSchedule.authorLabel") }}:
            {{
              s.authorMode === "me" ? t("reportSchedule.authorMe") : t("reportSchedule.authorAll")
            }}</span
          >
          <span class="max-w-48 truncate" :title="scheduleProjectNames(s.projectIds).join(', ')">
            {{ scheduleProjectNames(s.projectIds).slice(0, 2).join(", ")
            }}{{
              scheduleProjectNames(s.projectIds).length > 2
                ? ` +${scheduleProjectNames(s.projectIds).length - 2}`
                : ""
            }}
          </span>
          <span
            v-if="s.tagIds.length"
            class="flex items-center gap-1"
            :title="scheduleTagNames(s.tagIds).join(', ')"
          >
            <Tags class="h-3 w-3" />
            {{ scheduleTagNames(s.tagIds).join(", ") }}
          </span>
        </div>
        <div class="mt-1 text-[11px] text-muted-foreground">
          {{ t("reportSchedule.lastRun") }}: {{ lastRun(s.lastRunAt) }}
        </div>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :disabled="runningIds.includes(s.id)"
          :title="t('reportSchedule.runNow')"
          @click="emit('runNow', s)"
        >
          <Loader2 v-if="runningIds.includes(s.id)" class="h-3.5 w-3.5 animate-spin" />
          <Play v-else class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :title="s.enabled ? t('reportSchedule.enabled') : t('reportSchedule.disabled')"
          @click="emit('toggle', s)"
        >
          <Power v-if="s.enabled" class="h-3.5 w-3.5 text-green-500" />
          <PowerOff v-else class="h-3.5 w-3.5 text-muted-foreground" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :title="t('reportSchedule.edit')"
          @click="emit('edit', s)"
        >
          <Pencil class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 text-destructive hover:text-destructive"
          :title="t('reportSchedule.delete')"
          @click="emit('remove', s.id)"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  </div>
</template>
