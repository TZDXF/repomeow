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

const cacheTokens = computed(() => usage.value?.cachedInputTokens ?? 0);

const cacheCostText = computed(() => {
  const usd = estimateCost(cacheTokens.value, cost.value?.cacheRead);
  return usd == null ? undefined : formatUsd(usd);
});
</script>

<template>
  <slot v-if="$slots.default" />
  <div
    v-else-if="cacheTokens > 0"
    :class="cn('flex items-center justify-between text-xs', props.class)"
    v-bind="$attrs"
  >
    <span class="text-muted-foreground">{{ t("chat.context.cached") }}</span>
    <TokensWithCost :tokens="cacheTokens" :cost-text="cacheCostText" />
  </div>
</template>
