<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { EChartsCoreOption } from "echarts/core";
import EChart from "@/components/common/EChart.vue";
import { useChartTheme } from "@/composables/useChartTheme";
import { buildChurnCandles } from "@/lib/git-stats";
import type { GitDayStat } from "@/types";

const props = defineProps<{
  byDay: GitDayStat[];
}>();

const { t } = useI18n();
const { themeStamp } = useChartTheme();

/** 最近一年按周聚合的 K 线蜡烛:实体 0~净变更,影线 −deletions~additions */
const candles = computed(() => buildChurnCandles(props.byDay));

/** 阳线(净新增)/阴线(净减少)配色,沿用旧版 emerald/rose 语义 */
const UP_COLOR = "#10b981";
const DOWN_COLOR = "#f43f5e";

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

/** 带符号行数:+120 / −80(负号与 churnCell 一致用 U+2212) */
function signed(n: number): string {
  return n > 0 ? `+${n}` : n < 0 ? `−${Math.abs(n)}` : "0";
}

/** Y 轴大数值缩写:12000 → 12k */
function abbrev(v: number): string {
  const abs = Math.abs(v);
  if (abs >= 10000) return `${Math.round(v / 1000)}k`;
  if (abs >= 1000) return `${(v / 1000).toFixed(1)}k`;
  return String(v);
}

const option = computed<EChartsCoreOption>(() => {
  // 主题(亮暗/皮肤)变化时重建配色
  void themeStamp.value;
  const axisColor = resolveCssColor("--muted-foreground", "#888888");
  const borderColor = resolveCssColor("--border", "#e5e5e5");
  const list = candles.value;
  return {
    animationDuration: 300,
    grid: { left: 4, right: 8, top: 12, bottom: 0, containLabel: true },
    tooltip: {
      trigger: "axis",
      axisPointer: { type: "cross", label: { show: false } },
      backgroundColor: resolveCssColor("--popover", "#ffffff"),
      borderColor,
      textStyle: { color: resolveCssColor("--popover-foreground", "#111111"), fontSize: 12 },
      formatter: (params: unknown) => {
        const first = (Array.isArray(params) ? params[0] : params) as
          | { dataIndex?: number }
          | undefined;
        const c = typeof first?.dataIndex === "number" ? list[first.dataIndex] : undefined;
        if (!c) return "";
        return [
          t("git.graph.analysis.churnCell", { week: c.day, adds: c.additions, dels: c.deletions }),
          t("git.graph.analysis.churnNet", { net: signed(c.close) }),
        ].join("<br/>");
      },
    },
    xAxis: {
      type: "category",
      data: list.map((c) => c.day),
      axisTick: { show: false },
      axisLine: { lineStyle: { color: borderColor } },
      axisLabel: { color: axisColor, hideOverlap: true, formatter: (v: string) => v.slice(5) },
    },
    yAxis: {
      type: "value",
      scale: true,
      axisLabel: { color: axisColor, formatter: (v: number) => abbrev(v) },
      splitLine: { lineStyle: { color: borderColor } },
    },
    series: [
      {
        type: "candlestick",
        data: list.map((c) => [c.open, c.close, c.low, c.high]),
        barMaxWidth: 12,
        itemStyle: {
          color: UP_COLOR,
          color0: DOWN_COLOR,
          borderColor: UP_COLOR,
          borderColor0: DOWN_COLOR,
        },
      },
    ],
  };
});
</script>

<template>
  <template v-if="candles.length">
    <div class="h-40">
      <EChart :option="option" />
    </div>
    <div class="mt-2 flex items-center justify-end gap-3 text-[10px] text-muted-foreground">
      <span class="flex items-center gap-1">
        <span class="h-2.5 w-2.5 rounded-[2px]" :style="{ backgroundColor: UP_COLOR }" />
        {{ t("git.graph.analysis.churnUp") }}
      </span>
      <span class="flex items-center gap-1">
        <span class="h-2.5 w-2.5 rounded-[2px]" :style="{ backgroundColor: DOWN_COLOR }" />
        {{ t("git.graph.analysis.churnDown") }}
      </span>
    </div>
  </template>
  <p v-else class="py-6 text-center text-xs text-muted-foreground">
    {{ t("git.graph.analysis.empty") }}
  </p>
</template>
