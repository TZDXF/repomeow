<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { aggregateWeeks } from "@/lib/git-stats";
import type { GitDayStat } from "@/types";

const props = defineProps<{
  byDay: GitDayStat[];
}>();

const { t } = useI18n();

/** 最近一年按周聚合的增删行(上下对称:新增向上绿条,删除向下红条) */
const weeks = computed(() => aggregateWeeks(props.byDay));
const maxLines = computed(() =>
  Math.max(...weeks.value.flatMap((w) => [w.additions, w.deletions]), 0),
);

function barHeight(lines: number): string {
  if (lines <= 0 || maxLines.value <= 0) return "0%";
  return `${Math.max((lines / maxLines.value) * 100, 3)}%`;
}
</script>

<template>
  <template v-if="weeks.length">
    <div class="flex h-28 items-stretch gap-[2px]">
      <div
        v-for="week in weeks"
        :key="week.day"
        class="flex h-full min-w-0 w-1 flex-1 flex-col"
        :title="
          t('git.graph.analysis.churnCell', {
            week: week.day,
            adds: week.additions,
            dels: week.deletions,
          })
        "
      >
        <div class="flex flex-1 flex-col justify-end">
          <div
            class="rounded-t-sm bg-emerald-500/80 transition-colors hover:bg-emerald-500"
            :style="{ height: barHeight(week.additions) }"
          />
        </div>
        <div class="h-px bg-border" />
        <div class="flex flex-1 flex-col justify-start">
          <div
            class="rounded-b-sm bg-rose-500/80 transition-colors hover:bg-rose-500"
            :style="{ height: barHeight(week.deletions) }"
          />
        </div>
      </div>
    </div>
    <div class="mt-2 flex items-center justify-end gap-3 text-[10px] text-muted-foreground">
      <span class="flex items-center gap-1">
        <span class="h-2.5 w-2.5 rounded-[2px] bg-emerald-500/80" />
        {{ t("git.graph.analysis.churnAdds") }}
      </span>
      <span class="flex items-center gap-1">
        <span class="h-2.5 w-2.5 rounded-[2px] bg-rose-500/80" />
        {{ t("git.graph.analysis.churnDels") }}
      </span>
    </div>
  </template>
  <p v-else class="py-6 text-center text-xs text-muted-foreground">
    {{ t("git.graph.analysis.empty") }}
  </p>
</template>
