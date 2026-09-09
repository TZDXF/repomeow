<script setup lang="ts" generic="T extends { day: string; level: number; future: boolean }">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import HeatmapGrid from "@/components/common/HeatmapGrid.vue";

const props = withDefaults(
  defineProps<{
    weeks: T[][];
    monthLabels: { col: number; month: number }[];
    cellTitle: (cell: T) => string;
    centered?: boolean;
    showLegend?: boolean;
  }>(),
  { centered: false, showLegend: false },
);
const { t } = useI18n();
const weekdayKeys = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;
const rowLabels = computed(() =>
  weekdayKeys.map((key, row) => (row % 2 === 0 ? t(`settings.usage.weekdayShort.${key}`) : "")),
);
const columnLabels = computed(() => {
  const months = new Map(props.monthLabels.map(({ col, month }) => [col, month]));
  return props.weeks.map((_, col) =>
    months.has(col) ? t(`settings.usage.monthShort.m${months.get(col)}`) : "",
  );
});
const rows = computed(() =>
  weekdayKeys.map((_, row) =>
    props.weeks.flatMap((week) => {
      const cell = week[row];
      return cell
        ? [
            {
              key: cell.day,
              level: cell.level,
              hidden: cell.future,
              title: cell.future ? undefined : props.cellTitle(cell),
            },
          ]
        : [];
    }),
  ),
);
</script>

<template>
  <HeatmapGrid
    :rows="rows"
    :row-labels="rowLabels"
    :column-labels="columnLabels"
    :centered="centered"
    :legend="
      showLegend
        ? { less: t('settings.usage.legendLess'), more: t('settings.usage.legendMore') }
        : undefined
    "
  />
</template>
