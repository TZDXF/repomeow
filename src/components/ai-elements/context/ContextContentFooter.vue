<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { cn } from "@/lib/utils";
import { estimateCost, formatUsd, useContextValue } from "./context";

const props = defineProps<{
  class?: HTMLAttributes["class"];
}>();

const { t } = useI18n();
const { usage, cost } = useContextValue();

// 各分项成本与用量行口径一致(思考按输出计费);无费率或无用量时不渲染,避免误导性的 $0.00
const totalCost = computed(() => {
  const rates = cost.value;
  const current = usage.value;
  if (!rates || !current) return null;
  const parts = [
    estimateCost(current.inputTokens ?? 0, rates.input),
    estimateCost(current.outputTokens ?? 0, rates.output),
    estimateCost(current.reasoningTokens ?? 0, rates.output),
    estimateCost(current.cachedInputTokens ?? 0, rates.cacheRead),
  ];
  const usd = parts.reduce<number>((sum, part) => sum + (part ?? 0), 0);
  return usd > 0 ? formatUsd(usd) : null;
});
</script>

<template>
  <div
    v-if="$slots.default || totalCost"
    :class="
      cn('flex w-full items-center justify-between gap-3 bg-secondary p-3 text-xs', props.class)
    "
  >
    <slot v-if="$slots.default" />
    <template v-else>
      <span class="text-muted-foreground">{{ t("chat.context.totalCost") }}</span>
      <span class="tabular-nums">{{ totalCost }}</span>
    </template>
  </div>
</template>
