<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { cn } from "@/lib/utils";
import { estimateCost, formatUsd, useContextValue } from "./context";
import TokensWithCost from "./TokensWithCost.vue";

const props = defineProps<{
  class?: HTMLAttributes["class"];
}>();

const { t } = useI18n();
const { usage, cost } = useContextValue();

const reasoningTokens = computed(() => usage.value?.reasoningTokens ?? 0);

// 思考 token 计入输出计费
const reasoningCostText = computed(() => {
  const usd = estimateCost(reasoningTokens.value, cost.value?.output);
  return usd == null ? undefined : formatUsd(usd);
});
</script>

<template>
  <slot v-if="$slots.default" />
  <div
    v-else-if="reasoningTokens > 0"
    :class="cn('flex items-center justify-between text-xs', props.class)"
    v-bind="$attrs"
  >
    <span class="text-muted-foreground">{{ t("chat.context.reasoning") }}</span>
    <TokensWithCost :tokens="reasoningTokens" :cost-text="reasoningCostText" />
  </div>
</template>
