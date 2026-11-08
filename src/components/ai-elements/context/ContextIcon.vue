<script setup lang="ts">
import { computed } from "vue";
import { useContextValue } from "./context";

const ICON_RADIUS = 10,
  ICON_VIEWBOX = 24,
  ICON_CENTER = 12,
  ICON_STROKE_WIDTH = 3.5,
  ICON_SIZE = 16,
  { usedTokens, maxTokens } = useContextValue(),
  circumference = 2 * Math.PI * ICON_RADIUS,
  usedPercent = computed(() => {
    if (usedTokens.value == null || !maxTokens.value) {
      return 0;
    }
    return Math.min(1, usedTokens.value / maxTokens.value);
  }),
  dashOffset = computed(() => circumference * (1 - usedPercent.value)),
  svgStyle = {
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
