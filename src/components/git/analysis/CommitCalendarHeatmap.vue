<script setup lang="ts">
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

const monthLabelByCol = computed(() => {
  const map = new Map<number, number>();
  for (const { col, month } of calendar.value.monthLabels) map.set(col, month);
  return map;
});

/** 强度档配色:0 档为空底,1-4 档按 primary 透明度递增(与设置页用量热力图一致) */
const LEVEL_CLASSES = [
  "bg-muted",
  "bg-primary/25",
  "bg-primary/45",
  "bg-primary/70",
  "bg-primary",
] as const;

function levelClass(level: number): string {
  return LEVEL_CLASSES[level] ?? LEVEL_CLASSES[0];
}

/** 星期标签列只在一/三/五行显示文字,其余行占位对齐 */
function weekdayLabel(row: number): string {
  const keys = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;
  return row % 2 === 0 ? t(`settings.usage.weekdayShort.${keys[row]}`) : "";
}

function cellTitle(cell: CommitCalendarCell): string {
  if (cell.count === 0) {
    return t("git.graph.analysis.calendarEmpty", { date: cell.day });
  }
  return t("git.graph.analysis.calendarCell", { date: cell.day, count: cell.count });
}
</script>

<template>
  <div class="overflow-x-auto pb-1">
    <div class="flex w-max gap-1.5">
      <!-- 星期标签列:与格子行对齐(月份标签行高约 10px + 间距 4px) -->
      <div class="mt-[14px] flex w-4 shrink-0 flex-col gap-[3px]">
        <span
          v-for="row in 7"
          :key="row"
          class="flex h-3 items-center text-[10px] leading-none text-muted-foreground"
        >
          {{ weekdayLabel(row - 1) }}
        </span>
      </div>
      <div>
        <!-- 月份标签:标在包含当月 1 号的周列上方 -->
        <div class="flex gap-[3px]">
          <span
            v-for="col in calendar.weeks.length"
            :key="col"
            class="w-3 shrink-0 overflow-visible text-[10px] leading-none whitespace-nowrap text-muted-foreground"
          >
            {{
              monthLabelByCol.has(col - 1)
                ? t(`settings.usage.monthShort.m${monthLabelByCol.get(col - 1)}`)
                : ""
            }}
          </span>
        </div>
        <div class="mt-1 flex gap-[3px]">
          <div v-for="(week, col) in calendar.weeks" :key="col" class="flex flex-col gap-[3px]">
            <div
              v-for="cell in week"
              :key="cell.day"
              class="h-3 w-3 rounded-[2px]"
              :class="
                cell.future
                  ? 'invisible'
                  : `${levelClass(cell.level)} hover:ring-1 hover:ring-foreground/40`
              "
              :title="cell.future ? undefined : cellTitle(cell)"
            />
          </div>
        </div>
      </div>
    </div>
    <!-- 图例 -->
    <div class="mt-1.5 flex items-center justify-end gap-1 text-[10px] text-muted-foreground">
      <span>{{ t("settings.usage.legendLess") }}</span>
      <span
        v-for="lvl in 5"
        :key="lvl"
        class="h-2.5 w-2.5 rounded-[2px]"
        :class="levelClass(lvl - 1)"
      />
      <span>{{ t("settings.usage.legendMore") }}</span>
    </div>
  </div>
</template>
