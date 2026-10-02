<script setup lang="ts">
import { computed } from "vue";
import {
  Context,
  ContextBreakdownUsage,
  ContextCacheHitRate,
  ContextContent,
  ContextContentBody,
  ContextContentFooter,
  ContextContentHeader,
  ContextTrigger,
  type ContextUsage,
} from "@/components/ai-elements/context";
import { useAiConfigStore } from "@/stores/ai-config";
import type { ChatSessionState } from "@/stores/chat";

// ── 上下文占用弹层:窗口来自所选模型元数据;明细来自最近一次整轮用量 ──
const props = defineProps<{
  session: ChatSessionState;
}>();
const aiConfig = useAiConfigStore();

const contextWindow = computed(() => {
  const window = aiConfig.chatModel?.model.contextWindow;
  return window && window > 0 ? window : null;
});

// 上一轮用量明细(context 弹层的 Input/Output/Cache 行);成本费率取自模型元数据
const contextUsage = computed<ContextUsage | undefined>(() => {
  const usage = props.session.lastUsage;
  if (!usage) return undefined;
  return {
    inputTokens: usage.inputTokens,
    outputTokens: usage.outputTokens,
    cachedInputTokens: usage.cachedTokens ?? undefined,
  };
});

const contextCost = computed(() => aiConfig.chatModel?.model.cost ?? null);

// 平均缓存命中率:各轮加权(Σcached / Σinput);尚无样本时不展示
const cacheHitRate = computed(() => {
  const input = props.session.cacheHitInputTokens;
  if (input <= 0) return null;
  return props.session.cacheHitCachedTokens / input;
});
</script>

<template>
  <!-- 上下文占用:首个回答产出用量数据前不显示 -->
  <Context
    v-if="session.contextTokens != null"
    :used-tokens="session.contextTokens"
    :max-tokens="contextWindow"
    :usage="contextUsage"
    :cost="contextCost"
    :breakdown="session.contextBreakdown"
    :cache-hit-rate="cacheHitRate"
  >
    <ContextTrigger />
    <ContextContent side="top" align="end">
      <ContextContentHeader />
      <ContextContentBody class="space-y-2">
        <ContextBreakdownUsage />
        <ContextCacheHitRate />
      </ContextContentBody>
      <ContextContentFooter />
    </ContextContent>
  </Context>
</template>
