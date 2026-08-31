<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { computed } from "vue";
import { formatTokenCount } from "@/lib/chat";
import { cn } from "@/lib/utils";
import { contextPercent, formatContextPercent, useContextValue } from "./context";

const props = defineProps<{
  class?: HTMLAttributes["class"];
}>();

const { usedTokens, maxTokens } = useContextValue();

const percent = computed(() => contextPercent(usedTokens.value, maxTokens.value));
const displayPct = computed(() =>
  percent.value != null ? formatContextPercent(percent.value) : "—",
);
const used = computed(() => (usedTokens.value == null ? "—" : formatTokenCount(usedTokens.value)));
const total = computed(() =>
  maxTokens.value && maxTokens.value > 0 ? formatTokenCount(maxTokens.value) : null,
);

/** 占用分档着色:<70% 常规、<90% 提醒、其余警示 */
const barTone = computed(() => {
  const value = (percent.value ?? 0) * 100;
  if (value >= 90) return "bg-destructive";
  if (value >= 70) return "bg-amber-500";
  return "bg-primary";
});

const barWidth = computed(() =>
  percent.value == null || percent.value <= 0 ? "0%" : `${Math.max(2, percent.value * 100)}%`,
);
</script>

<template>
  <div :class="cn('w-full space-y-2 p-3', props.class)">
    <slot v-if="$slots.default" />
    <template v-else>
      <div class="flex items-center justify-between gap-3 text-xs">
        <p class="tabular-nums">{{ displayPct }}</p>
        <p class="font-mono text-muted-foreground tabular-nums">
          {{ used }}<template v-if="total"> / {{ total }}</template>
        </p>
      </div>
      <div class="bg-muted h-1.5 w-full overflow-hidden rounded-full">
        <div
          class="h-full rounded-full transition-[width]"
          :class="barTone"
          :style="{ width: barWidth }"
        />
      </div>
    </template>
  </div>
</template>
