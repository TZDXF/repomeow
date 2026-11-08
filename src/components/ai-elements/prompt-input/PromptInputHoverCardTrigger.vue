<script setup lang="ts">
import { HoverCardTrigger } from "@/components/ui/hover-card";

type PromptInputHoverCardTriggerProps = InstanceType<typeof HoverCardTrigger>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ PromptInputHoverCardTriggerProps {}

const props = defineProps<Props>();
</script>

<template>
  <HoverCardTrigger v-bind="props">
    <slot />
  </HoverCardTrigger>
</template>
