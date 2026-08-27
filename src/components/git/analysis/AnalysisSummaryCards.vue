<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { formatCompactNumber, formatRelativeTime } from "@/lib/format";
import type { GitProjectStats } from "@/types";

const props = defineProps<{
  stats: GitProjectStats;
}>();

const { t } = useI18n();

const fmt = (n: number) => formatCompactNumber(n);

/** 日均提交按有提交的天数平均(更能反映开发强度) */
const avgPerActiveDay = computed(() => {
  if (!props.stats.activeDays) return "0";
  return (props.stats.totalCommits / props.stats.activeDays).toFixed(1);
});

const cells = computed(() => [
  { label: t("git.graph.analysis.totalCommits"), value: fmt(props.stats.totalCommits) },
  { label: t("git.graph.analysis.authors"), value: fmt(props.stats.authors.length) },
  { label: t("git.graph.analysis.activeDays"), value: fmt(props.stats.activeDays) },
  {
    label: t("git.graph.analysis.avgPerDay"),
    value: avgPerActiveDay.value,
    title: t("git.graph.analysis.avgPerDayHint"),
  },
]);
</script>

<template>
  <div class="grid grid-cols-3 gap-2 xl:grid-cols-6">
    <div v-for="cell in cells" :key="cell.label" class="rounded-lg border px-3 py-2">
      <p class="truncate text-xs text-muted-foreground">{{ cell.label }}</p>
      <p class="mt-0.5 truncate text-base font-semibold tabular-nums" :title="cell.title">
        {{ cell.value }}
      </p>
    </div>
    <!-- 增删行单独一格:双色展示;churn 截断时给提示 -->
    <div
      class="rounded-lg border px-3 py-2"
      :title="stats.churnTruncated ? t('git.graph.analysis.churnTruncatedHint') : undefined"
    >
      <p class="truncate text-xs text-muted-foreground">
        {{ t("git.graph.analysis.churn") }}
      </p>
      <p class="mt-0.5 truncate text-base font-semibold tabular-nums">
        <span class="text-emerald-600 dark:text-emerald-400">+{{ fmt(stats.totalAdditions) }}</span>
        <span class="mx-0.5 text-muted-foreground">/</span>
        <span class="text-rose-600 dark:text-rose-400">−{{ fmt(stats.totalDeletions) }}</span>
      </p>
    </div>
    <div class="rounded-lg border px-3 py-2">
      <p class="truncate text-xs text-muted-foreground">
        {{ t("git.graph.analysis.lastCommit") }}
      </p>
      <p class="mt-0.5 truncate text-base font-semibold">
        {{ formatRelativeTime(stats.lastCommitAt) }}
      </p>
    </div>
  </div>
</template>
