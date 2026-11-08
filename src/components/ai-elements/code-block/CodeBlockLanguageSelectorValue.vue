<script setup lang="ts">
import { SelectValue } from "@/components/ui/select";

type CodeBlockLanguageSelectorValueProps = InstanceType<typeof SelectValue>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ CodeBlockLanguageSelectorValueProps {}

const props = defineProps<Props>();
</script>

<template>
  <SelectValue v-bind="{ ...props, ...$attrs }">
    <slot />
  </SelectValue>
</template>
