<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { useI18n } from "vue-i18n";
import { cn } from "@/lib/utils";
import { formatContextPercent, useContextValue } from "./context";

/** 平均缓存命中率行(各轮 Σcached / Σinput);无样本时不渲染 */
const props = defineProps<{
  class?: HTMLAttributes["class"];
}>();

const { t } = useI18n();
const { cacheHitRate } = useContextValue();
</script>

<template>
  <slot v-if="$slots.default" />
  <div
    v-else-if="cacheHitRate != null"
    :class="cn('flex items-center justify-between text-xs', props.class)"
    v-bind="$attrs"
  >
    <span class="text-muted-foreground">{{ t("chat.context.cacheHitRate") }}</span>
    <span class="tabular-nums">{{ formatContextPercent(cacheHitRate) }}</span>
  </div>
</template>
