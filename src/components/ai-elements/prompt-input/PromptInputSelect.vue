<script setup lang="ts">
import { Select } from "@/components/ui/select";

type PromptInputSelectProps = InstanceType<typeof Select>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ PromptInputSelectProps {}

const props = defineProps<Props>();
</script>

<template>
  <Select v-bind="props">
    <slot />
  </Select>
</template>
