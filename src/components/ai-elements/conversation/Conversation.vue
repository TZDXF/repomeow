<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { reactiveOmit } from "@vueuse/core";
import { StickToBottom } from "vue-stick-to-bottom";
import ConversationBottomPin from "./ConversationBottomPin.vue";

interface Props {
  ariaLabel?: string;
  class?: HTMLAttributes["class"];
  initial?: boolean | "instant" | { damping?: number; stiffness?: number; mass?: number };
  resize?: "instant" | { damping?: number; stiffness?: number; mass?: number };
  damping?: number;
  stiffness?: number;
  mass?: number;
  anchor?: "auto" | "none";
}

const props = withDefaults(defineProps<Props>(), {
  ariaLabel: "Conversation",
  initial: true,
  damping: 0.7,
  stiffness: 0.05,
  mass: 1.25,
  anchor: "none",
});
const delegatedProps = reactiveOmit(props, "class");
</script>

<template>
  <StickToBottom
    v-bind="delegatedProps"
    :class="cn('relative flex-1 overflow-y-hidden', props.class)"
    role="log"
  >
    <!-- 流式内容增长时在 paint 前同步钉住 scrollTop,消除库 rAF 修正的滚动条弹跳 -->
    <ConversationBottomPin />
    <slot />
  </StickToBottom>
</template>
