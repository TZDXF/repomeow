<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { formatTokenCount } from "@/lib/chat";
import { cn } from "@/lib/utils";
import { formatContextPercent, useContextValue } from "./context";

/**
 * 上下文构成占比行(系统提示词/工具定义/消息):各部分 token 数 +
 * 占当前已用上下文的比例;breakdown 缺失或全零时不渲染。
 */
const props = defineProps<{
  class?: HTMLAttributes["class"];
}>();

const { t } = useI18n();
const { breakdown, usedTokens } = useContextValue();

const parts = computed(() => {
  const current = breakdown.value;
  if (!current) return [];
  return [
    { key: "systemPrompt", label: t("chat.context.systemPrompt"), tokens: current.systemPrompt },
    { key: "tools", label: t("chat.context.tools"), tokens: current.tools },
    { key: "messages", label: t("chat.context.messages"), tokens: current.messages },
  ].filter((part) => part.tokens > 0);
});

function share(tokens: number): string | null {
  const used = usedTokens.value;
  if (used == null || used <= 0) return null;
  return formatContextPercent(tokens / used);
}
</script>

<template>
  <slot v-if="$slots.default" />
  <template v-else>
    <div
      v-for="part in parts"
      :key="part.key"
      :class="cn('flex items-center justify-between text-xs', props.class)"
    >
      <span class="text-muted-foreground">{{ part.label }}</span>
      <span class="tabular-nums">
        {{ formatTokenCount(part.tokens)
        }}<span v-if="share(part.tokens)" class="text-muted-foreground">
          · {{ share(part.tokens) }}</span
        >
      </span>
    </div>
  </template>
</template>
