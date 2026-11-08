<script setup lang="ts">
import type { ContextMenuRootEmits, ContextMenuRootProps } from "reka-ui";
import { ContextMenuRoot, useForwardPropsEmits } from "reka-ui";

const props = defineProps<ContextMenuRootProps>(),
  emits = defineEmits<ContextMenuRootEmits>(),
  forwarded = useForwardPropsEmits(props, emits);
</script>

<template>
  <ContextMenuRoot v-slot="slotProps" data-slot="context-menu" v-bind="forwarded">
    <slot v-bind="slotProps" />
  </ContextMenuRoot>
</template>
