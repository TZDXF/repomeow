<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { cn } from "@/lib/utils";
import { estimateCost, formatUsd, useContextValue } from "./context";
import TokensWithCost from "./TokensWithCost.vue";

const props = defineProps<{
    class?: HTMLAttributes["class"];
  }>(),
  { t } = useI18n(),
  { usage, cost } = useContextValue(),
  outputTokens = computed(() => usage.value?.outputTokens ?? 0),
  outputCostText = computed(() => {
    const usd = estimateCost(outputTokens.value, cost.value?.output);
    return usd == null ? undefined : formatUsd(usd);
  });
</script>

<template>
  <slot v-if="$slots.default" />
  <div
    v-else-if="outputTokens > 0"
    :class="cn('flex items-center justify-between text-xs', props.class)"
    v-bind="$attrs"
  >
    <span class="text-muted-foreground">{{ t("chat.context.output") }}</span>
    <TokensWithCost :tokens="outputTokens" :cost-text="outputCostText" />
  </div>
</template>
