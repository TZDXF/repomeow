<script setup lang="ts">
import HeatmapGrid from "@/components/common/HeatmapGrid.vue";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { heatLevel, weekdayHourAt } from "@/lib/git-stats";

const props = defineProps<{
  /** 7*24 行主序:行 = 周一..周日,列 = 0..23 时(提交者本地时间) */
  weekdayHour: number[];
}>();

const { t } = useI18n();

const WEEKDAY_KEYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;
/** 小时刻度:每 3 小时一个标签 */
const HOUR_TICKS = [0, 3, 6, 9, 12, 15, 18, 21] as const;

const maxCount = computed(() => Math.max(...props.weekdayHour.map(Number), 0));

const rowLabels = computed(() =>
  WEEKDAY_KEYS.map((key) => t(`settings.usage.weekdayShort.${key}`)),
);
const columnLabels = Array.from({ length: 24 }, (_, hour) =>
  (HOUR_TICKS as readonly number[]).includes(hour) ? String(hour) : "",
);
const rows = computed(() =>
  WEEKDAY_KEYS.map((_, weekday) =>
    Array.from({ length: 24 }, (_, hour) => ({
      key: `${weekday}-${hour}`,
      level: heatLevel(weekdayHourAt(props.weekdayHour, weekday, hour), maxCount.value),
      title: cellTitle(weekday, hour),
    })),
  ),
);

function cellTitle(weekday: number, hour: number): string {
  return t("git.graph.analysis.hourCell", {
    weekday: t(`settings.usage.weekdayShort.${WEEKDAY_KEYS[weekday]}`),
    hour: String(hour).padStart(2, "0"),
    count: weekdayHourAt(props.weekdayHour, weekday, hour),
  });
}
</script>

<template>
  <HeatmapGrid
    :rows="rows"
    :row-labels="rowLabels"
    :column-labels="columnLabels"
    size="md"
    centered
  />
</template>
