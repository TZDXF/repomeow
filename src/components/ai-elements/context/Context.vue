<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatTokenCount } from "@/lib/chat";

/** 上一轮回合的用量明细(展示在弹层;null = 尚无完成的回合) */
export interface ContextLastUsage {
  inputTokens: number;
  outputTokens: number;
  cachedTokens: number | null;
}

/**
 * 上下文占用指示器:触发器为迷你进度条 + token 数,弹层展示
 * 已用/窗口与上一轮输入输出明细。usedTokens 为 null 时触发器禁用。
 */
const props = withDefaults(
  defineProps<{
    usedTokens: number | null;
    contextWindow: number | null;
    lastUsage?: ContextLastUsage | null;
  }>(),
  { lastUsage: null },
);

const { t } = useI18n();

const percent = computed(() => {
  const { usedTokens, contextWindow } = props;
  if (usedTokens == null || !contextWindow || contextWindow <= 0) return null;
  return Math.min(100, Math.round((usedTokens / contextWindow) * 100));
});

/** 占用分档着色:<70% 常规、<90% 提醒、其余警示 */
const barTone = computed(() => {
  const value = percent.value ?? 0;
  if (value >= 90) return "bg-destructive";
  if (value >= 70) return "bg-amber-500";
  return "bg-primary";
});

const triggerLabel = computed(() => {
  if (props.usedTokens == null) return "—";
  if (percent.value != null) return `${percent.value}%`;
  return formatTokenCount(props.usedTokens);
});

const usedLabel = computed(() =>
  props.usedTokens == null ? null : formatTokenCount(props.usedTokens),
);
const windowLabel = computed(() =>
  props.contextWindow && props.contextWindow > 0 ? formatTokenCount(props.contextWindow) : null,
);
</script>

<template>
  <Popover>
    <PopoverTrigger as-child>
      <button
        type="button"
        class="text-muted-foreground hover:bg-accent hover:text-foreground flex h-7 items-center gap-1.5 rounded-md px-1.5 text-xs transition-colors disabled:pointer-events-none disabled:opacity-50"
        :disabled="usedTokens == null"
        :title="t('chat.context.title')"
      >
        <span class="relative h-1 w-9 overflow-hidden rounded-full bg-muted">
          <span
            class="absolute inset-y-0 left-0 rounded-full transition-[width]"
            :class="barTone"
            :style="{ width: percent == null ? '0%' : `${Math.max(2, percent)}%` }"
          />
        </span>
        <span class="tabular-nums">{{ triggerLabel }}</span>
      </button>
    </PopoverTrigger>
    <PopoverContent align="end" class="w-60 p-3 text-xs">
      <p class="text-muted-foreground pb-2 font-medium">{{ t("chat.context.title") }}</p>
      <div class="flex items-baseline justify-between">
        <span class="tabular-nums">
          {{
            usedLabel != null
              ? t("chat.context.usedTokens", { used: usedLabel })
              : t("chat.context.noUsage")
          }}
        </span>
        <span v-if="windowLabel" class="text-muted-foreground tabular-nums">
          {{ t("chat.context.window", { total: windowLabel }) }}
        </span>
      </div>
      <div class="bg-muted mt-1.5 h-1.5 overflow-hidden rounded-full">
        <div
          class="h-full rounded-full transition-[width]"
          :class="barTone"
          :style="{ width: percent == null ? '0%' : `${Math.max(2, percent)}%` }"
        />
      </div>
      <template v-if="lastUsage">
        <div class="text-muted-foreground mt-2.5 flex flex-col gap-1">
          <span>{{ t("chat.context.lastTurn") }}</span>
          <span class="flex justify-between tabular-nums">
            <span>{{ t("chat.context.input") }}</span>
            <span>{{ formatTokenCount(lastUsage.inputTokens) }}</span>
          </span>
          <span class="flex justify-between tabular-nums">
            <span>{{ t("chat.context.output") }}</span>
            <span>{{ formatTokenCount(lastUsage.outputTokens) }}</span>
          </span>
          <span v-if="lastUsage.cachedTokens" class="flex justify-between tabular-nums">
            <span>{{ t("chat.context.cached") }}</span>
            <span>{{ formatTokenCount(lastUsage.cachedTokens) }}</span>
          </span>
        </div>
      </template>
    </PopoverContent>
  </Popover>
</template>
