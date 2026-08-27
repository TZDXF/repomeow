<script setup lang="ts">
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

const LEVEL_CLASSES = [
  "bg-muted",
  "bg-primary/25",
  "bg-primary/45",
  "bg-primary/70",
  "bg-primary",
] as const;

function cellClass(weekday: number, hour: number): string {
  return LEVEL_CLASSES[heatLevel(weekdayHourAt(props.weekdayHour, weekday, hour), maxCount.value)];
}

function cellTitle(weekday: number, hour: number): string {
  return t("git.graph.analysis.hourCell", {
    weekday: t(`settings.usage.weekdayShort.${WEEKDAY_KEYS[weekday]}`),
    hour: String(hour).padStart(2, "0"),
    count: weekdayHourAt(props.weekdayHour, weekday, hour),
  });
}
</script>

<template>
  <div class="overflow-x-auto pb-1">
    <div class="w-max">
      <!-- 小时刻度行:与格子列对齐(星期标签列宽 48px + 间距 3px) -->
      <div class="flex gap-[3px] pl-[51px]">
        <span
          v-for="hour in 24"
          :key="hour"
          class="w-4 shrink-0 overflow-visible text-center text-[10px] leading-none whitespace-nowrap text-muted-foreground"
        >
          {{ (HOUR_TICKS as readonly number[]).includes(hour) ? hour : "" }}
        </span>
      </div>
      <div class="mt-1 flex flex-col gap-[3px]">
        <div v-for="weekday in 7" :key="weekday" class="flex items-center gap-[3px]">
          <span class="w-12 shrink-0 pr-1.5 text-right text-[10px] text-muted-foreground">
            {{ t(`settings.usage.weekdayShort.${WEEKDAY_KEYS[weekday - 1]}`) }}
          </span>
          <div
            v-for="hour in 24"
            :key="hour"
            class="h-4 w-4 rounded-[2px] hover:ring-1 hover:ring-foreground/40"
            :class="cellClass(weekday - 1, hour - 1)"
            :title="cellTitle(weekday - 1, hour - 1)"
          />
        </div>
      </div>
    </div>
  </div>
</template>
