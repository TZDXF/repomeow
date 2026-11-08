<script setup lang="ts">
import { SelectContent } from "@/components/common/project-floating-content";

type CodeBlockLanguageSelectorContentProps = InstanceType<typeof SelectContent>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ CodeBlockLanguageSelectorContentProps {}

const props = withDefaults(defineProps<Props>(), {
  align: "end",
});
</script>

<template>
  <SelectContent :align="props.align" v-bind="$attrs">
    <slot />
  </SelectContent>
</template>
