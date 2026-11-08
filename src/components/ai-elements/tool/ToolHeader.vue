<script setup lang="ts">
import type { Component, HTMLAttributes } from "vue";
import {
  CheckCircleIcon,
  ChevronDownIcon,
  Loader2Icon,
  WrenchIcon,
  XCircleIcon,
} from "@lucide/vue";
import { CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { ToolState } from "./types";

// 改造说明:原实现按 AI SDK 的 ToolUIPart['type'] 推导工具名,
// 这里直接接收调用方传入的 title(工具名),保持纯展示。
// 状态表达从文字徽标(StatusBadge)瘦身为小号状态图标 + 无障碍文本,
// 单行紧凑排列,连续调用时不占视觉空间。
const props = defineProps<{
  title?: string;
  /** 标题后的行内摘要(主参数值),截断展示 */
  subtitle?: string;
  state: ToolState;
  class?: HTMLAttributes["class"];
}>();

const { t } = useI18n();

const label = computed(() => props.title ?? "");

const statusIcon = computed<Component>(() => {
  const icons: Record<ToolState, Component> = {
    "input-streaming": Loader2Icon,
    "input-available": Loader2Icon,
    "output-available": CheckCircleIcon,
    "output-error": XCircleIcon,
  };
  return icons[props.state];
});

const statusClass = computed(() => {
  const classes: Record<ToolState, string> = {
    "input-streaming": "animate-spin text-muted-foreground",
    "input-available": "animate-spin text-muted-foreground",
    "output-available": "text-green-600",
    "output-error": "text-red-600",
  };
  return classes[props.state];
});

const statusText = computed(() => {
  const labels: Record<ToolState, string> = {
    "input-streaming": t("chat.toolRunning"),
    "input-available": t("chat.toolRunning"),
    "output-available": t("chat.toolDone"),
    "output-error": t("chat.toolFailed"),
  };
  return labels[props.state];
});
</script>

<template>
  <CollapsibleTrigger
    :class="cn('flex w-full items-center justify-between gap-2 px-2 py-1.5', props.class)"
    :title="statusText"
    v-bind="$attrs"
  >
    <div class="flex min-w-0 items-center gap-1.5">
      <WrenchIcon class="size-3.5 shrink-0 text-muted-foreground" />
      <span class="shrink-0 font-medium text-xs">{{ label }}</span>
      <span v-if="props.subtitle" class="min-w-0 truncate text-muted-foreground text-xs">
        {{ props.subtitle }}
      </span>
      <component :is="statusIcon" class="size-3.5 shrink-0" :class="statusClass" />
    </div>
    <ChevronDownIcon
      class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
    />
  </CollapsibleTrigger>
</template>
