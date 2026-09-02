<!-- StatusBadge.vue -->
<script setup lang="ts">
import type { Component } from "vue";
import { CheckCircleIcon, CircleHelpIcon, CircleIcon, ClockIcon, XCircleIcon } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { ToolState } from "./types";

// 改造说明:状态标签改为 vue-i18n 词条(chat.toolRunning / toolDone / toolFailed),
// 替代原 AI SDK 状态映射里的硬编码英文。
const { t } = useI18n();

const props = defineProps<{
  state: ToolState;
}>();

const label = computed(() => {
  const labels: Record<ToolState, string> = {
    "input-streaming": t("chat.toolRunning"),
    "input-available": t("chat.toolRunning"),
    "awaiting-permission": t("chat.toolAwaitingPermission"),
    "output-available": t("chat.toolDone"),
    "output-error": t("chat.toolFailed"),
  };
  return labels[props.state];
});

const icon = computed<Component>(() => {
  const icons: Record<ToolState, Component> = {
    "input-streaming": CircleIcon,
    "input-available": ClockIcon,
    "awaiting-permission": CircleHelpIcon,
    "output-available": CheckCircleIcon,
    "output-error": XCircleIcon,
  };
  return icons[props.state];
});

const iconClass = computed(() => {
  const classes: Record<ToolState, string> = {
    "input-streaming": "size-4",
    "input-available": "size-4 animate-pulse",
    "awaiting-permission": "size-4 text-amber-500",
    "output-available": "size-4 text-green-600",
    "output-error": "size-4 text-red-600",
  };
  return classes[props.state];
});
</script>

<template>
  <Badge class="gap-1.5 rounded-full text-xs" variant="secondary">
    <component :is="icon" :class="iconClass" />
    <span>{{ label }}</span>
  </Badge>
</template>
