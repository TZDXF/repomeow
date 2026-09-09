<script setup lang="ts">
import { computed } from "vue";
import type { EChartsCoreOption } from "echarts/core";
import EChart from "@/components/common/EChart.vue";
import { useChartTheme } from "@/composables/useChartTheme";

const props = withDefaults(
  defineProps<{
    /** 行主序网格；标签数组与对应行、列按索引对齐。 */
    rows: { key: string; level: number; title?: string; hidden?: boolean }[][];
    rowLabels: string[];
    columnLabels: string[];
    size?: "sm" | "md";
    centered?: boolean;
    legend?: { less: string; more: string };
  }>(),
  { size: "sm", centered: false, legend: undefined },
);
const { themeStamp } = useChartTheme();
const step = computed(() => (props.size === "sm" ? 15 : 19));
const left = computed(() => (props.size === "sm" ? 22 : 51));
const width = computed(() => left.value + props.columnLabels.length * step.value + 12);
const height = computed(() => 18 + props.rows.length * step.value);

/** 采样为 sRGB，避免 ECharts 不支持 oklch / color-mix 等主题颜色。 */
function resolveColor(css: string, fallback: string): string {
  const ctx = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  if (!ctx) return fallback;
  ctx.fillStyle = fallback;
  ctx.fillStyle = css;
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = ctx.getImageData(0, 0, 1, 1).data;
  return `rgba(${r}, ${g}, ${b}, ${a / 255})`;
}
const colors = computed(() => {
  void themeStamp.value;
  const style = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) =>
    resolveColor(style.getPropertyValue(name).trim(), fallback);
  const background = read("--background", "#ffffff");
  const foreground = read("--foreground", "#111111");
  const primary = read("--primary", "#0ea5e9");
  const mix = (color: string, amount: number) =>
    resolveColor(`color-mix(in srgb, ${color} ${amount}%, ${background})`, color);
  return {
    background,
    foreground,
    muted: read("--muted-foreground", "#888888"),
    popover: read("--popover", "#ffffff"),
    popoverForeground: read("--popover-foreground", "#111111"),
    // 零值不使用 muted：8bit 浅色主题的 muted 与背景相同。
    levels: [mix(foreground, 10), mix(primary, 25), mix(primary, 45), mix(primary, 70), primary],
  };
});
const option = computed<EChartsCoreOption>(() => {
  const palette = colors.value;
  const data = props.rows.flatMap((row, y) =>
    row.flatMap((cell, x) =>
      cell.hidden
        ? []
        : [
            {
              name: cell.title ?? "",
              value: [
                x,
                y,
                Number.isInteger(cell.level) && cell.level >= 0 && cell.level <= 4 ? cell.level : 0,
              ],
            },
          ],
    ),
  );
  return {
    animation: false,
    grid: { left: left.value, right: 12, top: 18, bottom: 0 },
    tooltip: {
      trigger: "item",
      renderMode: "richText",
      confine: true,
      backgroundColor: palette.popover,
      textStyle: { color: palette.popoverForeground, fontSize: 12 },
      formatter: (params: unknown) => (params as { name?: string }).name ?? "",
    },
    xAxis: {
      type: "category",
      position: "top",
      data: props.columnLabels.map((_, col) => String(col)),
      axisLine: { show: false },
      axisTick: { show: false },
      splitArea: { show: false },
      axisLabel: {
        interval: 0,
        fontSize: 10,
        margin: 5,
        color: palette.muted,
        formatter: (_: string, index: number) => props.columnLabels[index] ?? "",
      },
    },
    yAxis: {
      type: "category",
      inverse: true,
      data: props.rows.map((_, row) => String(row)),
      axisLine: { show: false },
      axisTick: { show: false },
      splitArea: { show: false },
      axisLabel: {
        interval: 0,
        fontSize: 10,
        margin: 6,
        color: palette.muted,
        formatter: (_: string, index: number) => props.rowLabels[index] ?? "",
      },
    },
    visualMap: {
      type: "piecewise",
      show: false,
      dimension: 2,
      pieces: palette.levels.map((color, value) => ({ value, color })),
    },
    series: [
      {
        type: "heatmap",
        data,
        progressive: 0,
        itemStyle: { borderWidth: 3, borderColor: palette.background, borderRadius: 2 },
        emphasis: { itemStyle: { borderColor: palette.foreground, borderWidth: 1 } },
      },
    ],
  };
});
</script>

<template>
  <div class="overflow-x-auto pb-1">
    <div :class="{ 'mx-auto': centered }" :style="{ width: `${width}px` }">
      <div :style="{ height: `${height}px` }">
        <EChart :option="option" />
      </div>
      <div
        v-if="legend"
        class="mt-1.5 flex items-center justify-end gap-1 text-[10px] text-muted-foreground"
      >
        <span>{{ legend.less }}</span>
        <span
          v-for="(color, level) in colors.levels"
          :key="level"
          class="h-2.5 w-2.5 rounded-[2px]"
          :style="{ backgroundColor: color }"
        />
        <span>{{ legend.more }}</span>
      </div>
    </div>
  </div>
</template>
