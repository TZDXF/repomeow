<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { EChartsCoreOption } from "echarts/core";
import EChart from "@/components/common/EChart.vue";
import { useChartTheme } from "@/composables/useChartTheme";
import { formatLocalDateTime } from "@/lib/format";
import type { GitCommitChurn } from "@/types";

const props = defineProps<{
  /** 逐提交增删行(仅非合并提交),按 committer 时间升序 */
  commits: GitCommitChurn[];
}>();

const { t } = useI18n();
const { themeStamp } = useChartTheme();

/** 阳线(净新增)/阴线(净减少)配色,沿用旧版 emerald/rose 语义 */
const UP_COLOR = "#10b981";
const DOWN_COLOR = "#f43f5e";

/** 数据量大时 dataZoom 默认窗口只展示最近的蜡烛数(滚轮缩放/拖动平移查看更早) */
const DEFAULT_VISIBLE_CANDLES = 200;

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

/** tooltip 是 HTML 渲染,提交信息首行必须转义 */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
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
  const list = props.commits;
  const len = list.length;
  const startPercent =
    len > DEFAULT_VISIBLE_CANDLES ? (1 - DEFAULT_VISIBLE_CANDLES / len) * 100 : 0;
  // 金融 K 线形态:open 接上一根 close(累计净变更),蜡烛沿代码量走势起伏;
  // 影线 = 本次新增推高(+additions)/删除下探(−deletions)的位置
  let prev = 0;
  const candles = list.map((c) => {
    const open = prev;
    const close = prev + c.additions - c.deletions;
    const k = { t: c.t * 1000, open, close, low: prev - c.deletions, high: prev + c.additions };
    prev = close;
    return k;
  });
  return {
    animationDuration: 300,
    // 底部留出 dataZoom 滑条的位置
    grid: { left: 4, right: 8, top: 12, bottom: 22, containLabel: true },
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
        const idx = typeof first?.dataIndex === "number" ? first.dataIndex : -1;
        const c = list[idx];
        const k = candles[idx];
        if (!c || !k) return "";
        const net = c.additions - c.deletions;
        return [
          `<div style="font-weight:600;max-width:360px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(c.subject)} <span style="opacity:.55;font-weight:400">#${c.shortId}</span></div>`,
          `<div style="opacity:.75">${formatLocalDateTime(c.t)}</div>`,
          `<div>${t("git.graph.analysis.churnCell", { adds: c.additions, dels: c.deletions })} · ${t("git.graph.analysis.churnNet", { net: signed(net) })}</div>`,
          `<div style="opacity:.75">${t("git.graph.analysis.churnTotalLine", { total: signed(k.close) })}</div>`,
        ].join("");
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
      scale: true,
      axisLabel: { color: axisColor, formatter: (v: number) => abbrev(v) },
      splitLine: { lineStyle: { color: borderColor } },
    },
    dataZoom: [
      { type: "inside", start: startPercent, end: 100 },
      {
        type: "slider",
        start: startPercent,
        end: 100,
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
        type: "candlestick",
        // [时刻, open, close, low, high]:实体为前后两次提交的累计净变更,影线覆盖删除下探~新增推高
        data: candles.map((k) => [k.t, k.open, k.close, k.low, k.high]),
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
  <template v-if="commits.length">
    <div class="h-48">
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
