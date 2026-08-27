<script setup lang="ts">
/**
 * ECharts 通用容器:按需注册图表/组件,负责 init、option 更新、resize 与销毁。
 * 尺寸由父容器决定(自身 h-full w-full);option 变化时整体替换(notMerge)。
 * 语言包跟随应用语言(locale 只能在 init 时指定,切换语言后销毁重建实例)。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as echarts from "echarts/core";
import { CandlestickChart, LineChart } from "echarts/charts";
import {
  DataZoomInsideComponent,
  DataZoomSliderComponent,
  GridComponent,
  TooltipComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import langZH from "echarts/i18n/langZH-obj.js";
import type { EChartsCoreOption } from "echarts/core";
import { i18n } from "@/i18n";

echarts.use([
  CandlestickChart,
  LineChart,
  DataZoomInsideComponent,
  DataZoomSliderComponent,
  GridComponent,
  TooltipComponent,
  CanvasRenderer,
]);
// echarts 内置默认语言为英文;中文语言包按需注册(时间轴月份名等)
echarts.registerLocale("ZH", langZH);

const props = defineProps<{
  option: EChartsCoreOption;
}>();

const el = ref<HTMLElement>();
let chart: ReturnType<typeof echarts.init> | undefined;
let resizeObserver: ResizeObserver | undefined;

/** 应用语言 → echarts 语言包名("EN" 为内置默认,无需注册) */
const chartLocale = computed(() => (i18n.global.locale.value === "zh-CN" ? "ZH" : "EN"));

function createChart() {
  if (!el.value) return;
  chart?.dispose();
  chart = echarts.init(el.value, undefined, {
    renderer: "canvas",
    locale: chartLocale.value,
  });
  chart.setOption(props.option);
}

onMounted(() => {
  createChart();
  if (!el.value) return;
  resizeObserver = new ResizeObserver(() => chart?.resize());
  resizeObserver.observe(el.value);
});

watch(
  () => props.option,
  (option) => {
    chart?.setOption(option, { notMerge: true });
  },
);

// 语言切换后 locale 无法经 setOption 变更,销毁重建
watch(chartLocale, () => createChart());

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  chart?.dispose();
  chart = undefined;
});
</script>

<template>
  <div ref="el" class="h-full w-full" />
</template>
