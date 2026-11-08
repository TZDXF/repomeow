<script setup lang="ts">
import { HoverCardContent } from "@/components/ui/hover-card";

type PromptInputHoverCardContentProps = InstanceType<typeof HoverCardContent>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ PromptInputHoverCardContentProps {}

const props = withDefaults(defineProps<Props>(), {
    align: "start",
  }),
  { align, ...restProps } = props;
</script>

<template>
  <HoverCardContent :align="props.align" v-bind="restProps">
    <slot />
  </HoverCardContent>
</template>
