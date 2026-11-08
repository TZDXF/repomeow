<script setup lang="ts">
import { Select } from "@/components/ui/select";

type CodeBlockLanguageSelectorProps = InstanceType<typeof Select>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ CodeBlockLanguageSelectorProps {}

const props = defineProps<Props>();
</script>

<template>
  <Select v-bind="{ ...props, ...$attrs }">
    <slot />
  </Select>
</template>
