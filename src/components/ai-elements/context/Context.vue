<script setup lang="ts">
import { computed, provide } from "vue";
import { HoverCard } from "@/components/ui/hover-card";
import type { AiModelCost } from "@/lib/ai-config";
import { ContextKey, type ContextBreakdown, type ContextUsage } from "./context";

const props = defineProps<{
  usedTokens: number | null;
  maxTokens: number | null;
  usage?: ContextUsage;
  cost?: AiModelCost | null;
  breakdown?: ContextBreakdown | null;
  cacheHitRate?: number | null;
}>();

provide(ContextKey, {
  usedTokens: computed(() => props.usedTokens),
  maxTokens: computed(() => props.maxTokens),
  usage: computed(() => props.usage),
  cost: computed(() => props.cost),
  breakdown: computed(() => props.breakdown),
  cacheHitRate: computed(() => props.cacheHitRate),
});
</script>

<template>
  <HoverCard :open-delay="0" :close-delay="0">
    <slot />
  </HoverCard>
</template>
