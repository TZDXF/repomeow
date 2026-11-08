<script setup lang="ts">
import { SelectItem } from "@/components/ui/select";

type CodeBlockLanguageSelectorItemProps = InstanceType<typeof SelectItem>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ CodeBlockLanguageSelectorItemProps {}

const props = defineProps<Props>();
</script>

<template>
  <SelectItem v-bind="{ ...props, ...$attrs }">
    <slot />
  </SelectItem>
</template>
