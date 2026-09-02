<script setup lang="ts">
import { computed } from "vue";
import { useContextValue } from "./context";

const ICON_RADIUS = 10;
const ICON_VIEWBOX = 24;
const ICON_CENTER = 12;
const ICON_STROKE_WIDTH = 3.5;
const ICON_SIZE = 16;

const { usedTokens, maxTokens } = useContextValue();

const circumference = 2 * Math.PI * ICON_RADIUS;

const usedPercent = computed(() => {
  if (usedTokens.value == null || !maxTokens.value) return 0;
  return Math.min(1, usedTokens.value / maxTokens.value);
});

const dashOffset = computed(() => circumference * (1 - usedPercent.value));

const svgStyle = {
  transformOrigin: "center",
  transform: "rotate(-90deg)",
};
</script>

<template>
  <svg
    aria-label="Model context usage"
    :height="ICON_SIZE"
    role="img"
    style="color: currentcolor"
    :viewBox="`0 0 ${ICON_VIEWBOX} ${ICON_VIEWBOX}`"
    :width="ICON_SIZE"
  >
    <circle
      :cx="ICON_CENTER"
      :cy="ICON_CENTER"
      fill="none"
      opacity="0.25"
      :r="ICON_RADIUS"
      stroke="currentColor"
      :stroke-width="ICON_STROKE_WIDTH"
    />
    <circle
      :cx="ICON_CENTER"
      :cy="ICON_CENTER"
      fill="none"
      opacity="0.7"
      :r="ICON_RADIUS"
      stroke="currentColor"
      :stroke-dasharray="`${circumference} ${circumference}`"
      :stroke-dashoffset="dashOffset"
      stroke-linecap="round"
      :stroke-width="ICON_STROKE_WIDTH"
      :style="svgStyle"
    />
  </svg>
</template>
