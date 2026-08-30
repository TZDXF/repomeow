<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { ChevronDownIcon, WrenchIcon } from "@lucide/vue";
import { CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { computed } from "vue";
import StatusBadge from "./ToolStatusBadge.vue";
import type { ToolState } from "./types";

// 改造说明:原实现按 AI SDK 的 ToolUIPart['type'] 推导工具名,
// 这里直接接收调用方传入的 title(工具名),保持纯展示。
const props = defineProps<{
  title?: string;
  state: ToolState;
  class?: HTMLAttributes["class"];
}>();

const label = computed(() => props.title ?? "");
</script>

<template>
  <CollapsibleTrigger
    :class="cn('flex w-full items-center justify-between gap-4 p-3', props.class)"
    v-bind="$attrs"
  >
    <div class="flex items-center gap-2">
      <WrenchIcon class="size-4 text-muted-foreground" />
      <span class="font-medium text-sm">{{ label }}</span>
      <StatusBadge :state="props.state" />
    </div>
    <ChevronDownIcon
      class="size-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
    />
  </CollapsibleTrigger>
</template>
