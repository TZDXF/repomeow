<script setup lang="ts">
import type { PopoverRootEmits, PopoverRootProps } from "reka-ui";
import { PopoverRoot, useForwardPropsEmits } from "reka-ui";

const props = defineProps<PopoverRootProps>(),
  emits = defineEmits<PopoverRootEmits>(),
  forwarded = useForwardPropsEmits(props, emits);
</script>

<template>
  <PopoverRoot v-slot="slotProps" data-slot="popover" v-bind="forwarded">
    <slot v-bind="slotProps" />
  </PopoverRoot>
</template>
