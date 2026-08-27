<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { cmd } from "@/lib/tauri";
import type { GitProjectStats } from "@/types";
import AnalysisCard from "./AnalysisCard.vue";
import AnalysisSummaryCards from "./AnalysisSummaryCards.vue";
import AuthorStatsList from "./AuthorStatsList.vue";
import CommitCalendarHeatmap from "./CommitCalendarHeatmap.vue";
import FileTypeStats from "./FileTypeStats.vue";
import WeekdayHourHeatmap from "./WeekdayHourHeatmap.vue";
import WeeklyChurnChart from "./WeeklyChurnChart.vue";
import WeeklyTrendChart from "./WeeklyTrendChart.vue";

const props = defineProps<{
  /** 项目路径(分析对象) */
  path: string;
}>();

const { t } = useI18n();

const stats = ref<GitProjectStats | null>(null);
const loading = ref(false);
const error = ref("");
// 单调递增请求令牌:切换项目/连续刷新时丢弃过期响应(照抄 AiUsageSettings 的防竞态范式)
let loadToken = 0;

async function reload() {
  const token = ++loadToken;
  loading.value = true;
  error.value = "";
  try {
    const result = await cmd<GitProjectStats>("git_project_stats", { path: props.path });
    if (token !== loadToken) return;
    stats.value = result;
  } catch (e) {
    if (token === loadToken) {
      error.value = String(e);
    }
  } finally {
    if (token === loadToken) {
      loading.value = false;
    }
  }
}

onMounted(() => void reload());
watch(
  () => props.path,
  () => {
    stats.value = null;
    void reload();
  },
);

// 头部刷新按钮经 GitGraph 视图转发调用
defineExpose({ reload });

const churnHint = computed(() =>
  stats.value?.churnTruncated ? t("git.graph.analysis.churnTruncatedHint") : undefined,
);
</script>

<template>
  <div class="h-full overflow-auto">
    <!-- 加载中(首次:尚无数据) -->
    <div v-if="loading && !stats" class="flex h-full items-center justify-center">
      <Loader2 class="h-5 w-5 animate-spin text-muted-foreground" />
    </div>

    <!-- 加载失败 -->
    <div
      v-else-if="error && !stats"
      class="flex h-full flex-col items-center justify-center gap-3 text-sm text-muted-foreground"
    >
      <p>{{ t("git.graph.loadFailed") }}:{{ error }}</p>
      <Button variant="outline" size="sm" @click="reload">{{ t("git.graph.refresh") }}</Button>
    </div>

    <!-- 空仓库 -->
    <div
      v-else-if="stats && !stats.totalCommits"
      class="flex h-full items-center justify-center text-sm text-muted-foreground"
    >
      {{ t("git.graph.analysis.empty") }}
    </div>

    <div v-else-if="stats" class="mx-auto flex max-w-6xl flex-col gap-4 p-4">
      <AnalysisSummaryCards :stats="stats" />

      <AnalysisCard :title="t('git.graph.analysis.calendar')">
        <CommitCalendarHeatmap :by-day="stats.byDay" />
      </AnalysisCard>

      <div class="grid gap-4 xl:grid-cols-2">
        <AnalysisCard :title="t('git.graph.analysis.weekdayHour')">
          <WeekdayHourHeatmap :weekday-hour="stats.weekdayHour" />
        </AnalysisCard>
        <AnalysisCard :title="t('git.graph.analysis.trend')">
          <WeeklyTrendChart :by-day="stats.byDay" />
        </AnalysisCard>
      </div>

      <div class="grid gap-4 xl:grid-cols-2">
        <AnalysisCard :title="t('git.graph.analysis.authorsTitle')">
          <AuthorStatsList :authors="stats.authors" />
        </AnalysisCard>
        <AnalysisCard :title="t('git.graph.analysis.fileTypes')">
          <FileTypeStats :file-types="stats.fileTypes" :total-bytes="stats.totalBytes" />
        </AnalysisCard>
      </div>

      <AnalysisCard :title="t('git.graph.analysis.churnTrend')" :hint="churnHint">
        <WeeklyChurnChart :by-day="stats.byDay" />
      </AnalysisCard>
    </div>
  </div>
</template>
