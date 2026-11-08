<script setup lang="ts">
import type { TooltipRootEmits, TooltipRootProps } from "reka-ui";
import { TooltipRoot, useForwardPropsEmits } from "reka-ui";

const props = defineProps<TooltipRootProps>(),
  emits = defineEmits<TooltipRootEmits>(),
  forwarded = useForwardPropsEmits(props, emits);
</script>

<template>
  <TooltipRoot v-slot="slotProps" data-slot="tooltip" v-bind="forwarded">
    <slot v-bind="slotProps" />
  </TooltipRoot>
</template>
