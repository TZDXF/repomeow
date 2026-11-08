<script setup lang="ts">
import { DropdownMenu } from "@/components/ui/dropdown-menu";

type DropdownMenuProps = InstanceType<typeof DropdownMenu>["$props"];

// oxlint-disable-next-line typescript/no-empty-interface -- @vue-ignore 需要接口形式供 SFC 编译器解析。
interface Props extends /* @vue-ignore */ DropdownMenuProps {}

const props = defineProps<Props>();
</script>

<template>
  <DropdownMenu v-bind="props">
    <slot />
  </DropdownMenu>
</template>
