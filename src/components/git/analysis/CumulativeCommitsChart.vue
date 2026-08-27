<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { EChartsCoreOption } from "echarts/core";
import EChart from "@/components/common/EChart.vue";
import { useChartTheme } from "@/composables/useChartTheme";
import { formatCompactNumber } from "@/lib/format";
import { buildCumulativeCommits } from "@/lib/git-stats";
import type { GitDayStat } from "@/types";

const props = defineProps<{
  byDay: GitDayStat[];
}>();

const { t } = useI18n();
const { themeStamp } = useChartTheme();

/** 全历史累计提交曲线:横轴时间,纵轴截至当日的累计提交数 */
const points = computed(() => buildCumulativeCommits(props.byDay));

/**
 * 读根节点主题 CSS 变量并归一化为 rgb/#hex。
 * canvas 无法直接用 var();echarts/zrender 不解析 oklch,经 canvas fillStyle 归一化后使用。
 */
function resolveCssColor(name: string, fallback: string): string {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  if (!raw) return fallback;
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return fallback;
  ctx.fillStyle = raw;
  return ctx.fillStyle;
}

const option = computed<EChartsCoreOption>(() => {
  // 主题(亮暗/皮肤)变化时重建配色
  void themeStamp.value;
  const axisColor = resolveCssColor("--muted-foreground", "#888888");
  const borderColor = resolveCssColor("--border", "#e5e5e5");
  const primary = resolveCssColor("--primary", "#0ea5e9");
  const list = points.value;
  return {
    animationDuration: 300,
    // 底部留出 dataZoom 滑条的位置
    grid: { left: 4, right: 8, top: 12, bottom: 22, containLabel: true },
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "line", label: { show: false } },
      backgroundColor: resolveCssColor("--popover", "#ffffff"),
      borderColor,
      textStyle: { color: resolveCssColor("--popover-foreground", "#111111"), fontSize: 12 },
      formatter: (params: unknown) => {
        const first = (Array.isArray(params) ? params[0] : params) as
          | { dataIndex?: number }
          | undefined;
        const p = typeof first?.dataIndex === "number" ? list[first.dataIndex] : undefined;
        if (!p) return "";
        return t("git.graph.analysis.cumulativeCell", { day: p.day, count: p.total });
      },
    },
    xAxis: {
      type: "time",
      axisTick: { show: false },
      axisLine: { lineStyle: { color: borderColor } },
      axisLabel: { color: axisColor, hideOverlap: true },
    },
    yAxis: {
      type: "value",
      minInterval: 1,
      axisLabel: { color: axisColor, formatter: (v: number) => formatCompactNumber(v) },
      splitLine: { lineStyle: { color: borderColor } },
    },
    // 默认展示全历史(曲线的意义就在完整走势),滚轮缩放/拖动平移查看局部
    dataZoom: [
      { type: "inside" },
      {
        type: "slider",
        height: 14,
        bottom: 2,
        borderColor: "transparent",
        backgroundColor: "transparent",
        fillerColor: "rgba(128,128,128,0.15)",
        handleStyle: { color: axisColor },
        moveHandleStyle: { color: axisColor },
        dataBackground: {
          lineStyle: { color: axisColor },
          areaStyle: { color: axisColor, opacity: 0.1 },
        },
        selectedDataBackground: {
          lineStyle: { color: axisColor },
          areaStyle: { color: axisColor, opacity: 0.2 },
        },
        textStyle: { color: axisColor },
      },
    ],
    series: [
      {
        type: "line",
        data: list.map((p) => [p.t * 1000, p.total]),
        // 只有一个数据点时折线不可见,退回散点
        showSymbol: list.length === 1,
        symbol: "circle",
        symbolSize: 5,
        sampling: "lttb",
        lineStyle: { width: 2, color: primary },
        itemStyle: { color: primary },
        areaStyle: { color: primary, opacity: 0.12 },
      },
    ],
  };
});
</script>

<template>
  <div v-if="points.length" class="h-40">
    <EChart :option="option" />
  </div>
  <p v-else class="py-6 text-center text-xs text-muted-foreground">
    {{ t("git.graph.analysis.empty") }}
  </p>
</template>
