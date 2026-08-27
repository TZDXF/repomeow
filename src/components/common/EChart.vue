<script setup lang="ts">
/**
 * ECharts 通用容器:按需注册图表/组件,负责 init、option 更新、resize 与销毁。
 * 尺寸由父容器决定(自身 h-full w-full);option 变化时整体替换(notMerge)。
 */
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as echarts from "echarts/core";
import { CandlestickChart } from "echarts/charts";
import { GridComponent, TooltipComponent } from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import type { EChartsCoreOption } from "echarts/core";

echarts.use([CandlestickChart, GridComponent, TooltipComponent, CanvasRenderer]);

const props = defineProps<{
  option: EChartsCoreOption;
}>();

const el = ref<HTMLElement>();
let chart: ReturnType<typeof echarts.init> | undefined;
let resizeObserver: ResizeObserver | undefined;

onMounted(() => {
  if (!el.value) return;
  chart = echarts.init(el.value, undefined, { renderer: "canvas" });
  chart.setOption(props.option);
  resizeObserver = new ResizeObserver(() => chart?.resize());
  resizeObserver.observe(el.value);
});

watch(
  () => props.option,
  (option) => {
    chart?.setOption(option, { notMerge: true });
  },
);

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  chart?.dispose();
  chart = undefined;
});
</script>

<template>
  <div ref="el" class="h-full w-full" />
</template>
