<script setup lang="ts">
/**
 * ScrollArea 包装组件：承载视口高度修复，避免手改 ui/ 下的 shadcn 生成组件。
 *
 * 问题：生成版视口 size-full 的 h-full 百分比高度，在 flex + max-h 夹出的不确定父高度下
 * 解析失败、退化为内容高度（列表溢出弹窗且不滚动）。
 * 修复：根节点补 flex flex-col，视口以 flex 子项自适应高度（height:auto + min-height:0）
 * 收缩受控。scoped 样式不经 @layer，优先级高于 Tailwind 4 的 utilities 层，可覆盖 size-full。
 */
import type { ScrollAreaRootProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { cn } from "@/lib/utils";
import { ScrollArea as UiScrollArea } from "@/components/ui/scroll-area";

const props = defineProps<ScrollAreaRootProps & { class?: HTMLAttributes["class"] }>();

const delegatedProps = reactiveOmit(props, "class");
</script>

<template>
  <UiScrollArea v-bind="delegatedProps" :class="cn('flex flex-col', props.class)">
    <slot />
  </UiScrollArea>
</template>

<style scoped>
:deep([data-slot="scroll-area-viewport"]) {
  height: auto;
  min-height: 0;
}
</style>
