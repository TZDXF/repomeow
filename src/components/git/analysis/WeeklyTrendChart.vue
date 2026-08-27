<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { aggregateWeeks } from "@/lib/git-stats";
import type { GitDayStat } from "@/types";

const props = defineProps<{
  byDay: GitDayStat[];
}>();

const { t } = useI18n();

/** 最近一年按周聚合的提交数 */
const weeks = computed(() => aggregateWeeks(props.byDay));
const maxCount = computed(() => Math.max(...weeks.value.map((w) => w.count), 0));

function barHeight(count: number): string {
  // 有提交时保底 3% 高度,避免与空周难以区分
  if (count <= 0 || maxCount.value <= 0) return "0%";
  return `${Math.max((count / maxCount.value) * 100, 3)}%`;
}
</script>

<template>
  <div v-if="weeks.length" class="flex h-28 items-end gap-[2px]">
    <div
      v-for="week in weeks"
      :key="week.day"
      class="min-w-0 w-1 flex-1 rounded-t-sm bg-primary/70 transition-colors hover:bg-primary"
      :style="{ height: barHeight(week.count) }"
      :title="t('git.graph.analysis.trendCell', { week: week.day, count: week.count })"
    />
  </div>
  <p v-else class="py-6 text-center text-xs text-muted-foreground">
    {{ t("git.graph.analysis.empty") }}
  </p>
</template>
