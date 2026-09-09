<script setup lang="ts">
import CalendarHeatmap from "@/components/common/CalendarHeatmap.vue";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { buildCommitCalendar, type CommitCalendarCell } from "@/lib/git-stats";
import type { GitDayStat } from "@/types";

const props = defineProps<{
  byDay: GitDayStat[];
}>();

const { t } = useI18n();

/** 最近一年提交日历(周列 × 周一~周日行,GitHub 贡献图风格) */
const calendar = computed(() => buildCommitCalendar(props.byDay));

function cellTitle(cell: CommitCalendarCell): string {
  if (cell.count === 0) {
    return t("git.graph.analysis.calendarEmpty", { date: cell.day });
  }
  return t("git.graph.analysis.calendarCell", { date: cell.day, count: cell.count });
}
</script>

<template>
  <CalendarHeatmap
    :weeks="calendar.weeks"
    :month-labels="calendar.monthLabels"
    :cell-title="cellTitle"
    centered
  />
</template>
