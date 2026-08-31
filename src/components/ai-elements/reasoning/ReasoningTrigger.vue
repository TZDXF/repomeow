<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { BrainIcon, ChevronDownIcon } from "@lucide/vue";
import { CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { Shimmer } from "../shimmer";
import { useReasoningContext } from "./context";

// 改造说明:官方实现的三段文案(Thinking… / Thought for a few seconds /
// Thought for N seconds)硬编码英文,这里走 vue-i18n(同 ToolStatusBadge 先例)。
interface Props {
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();

const { t } = useI18n();
const { isStreaming, isOpen, duration } = useReasoningContext();

const thinkingMessage = computed(() => {
  if (isStreaming.value || duration.value === 0) return "thinking";
  if (duration.value === undefined) return "default_done";
  return "duration_done";
});
</script>

<template>
  <CollapsibleTrigger
    :class="
      cn(
        'flex w-full items-center gap-2 text-muted-foreground text-xs transition-colors hover:text-foreground',
        props.class,
      )
    "
  >
    <slot>
      <BrainIcon class="size-3.5" />

      <Shimmer v-if="thinkingMessage === 'thinking'" :duration="1">
        {{ t("chat.reasoning.thinking") }}
      </Shimmer>
      <p v-else-if="thinkingMessage === 'default_done'">
        {{ t("chat.reasoning.done") }}
      </p>
      <p v-else>{{ t("chat.reasoning.doneSeconds", { seconds: duration }) }}</p>

      <ChevronDownIcon
        :class="cn('size-3.5 transition-transform', isOpen ? 'rotate-180' : 'rotate-0')"
      />
    </slot>
  </CollapsibleTrigger>
</template>
