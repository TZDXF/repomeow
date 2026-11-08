<script setup lang="ts">
import type { CollapsibleRootEmits, CollapsibleRootProps } from "reka-ui";
import { CollapsibleRoot, useForwardPropsEmits } from "reka-ui";

const props = defineProps<CollapsibleRootProps>(),
  emits = defineEmits<CollapsibleRootEmits>(),
  forwarded = useForwardPropsEmits(props, emits);
</script>

<template>
  <CollapsibleRoot v-slot="{ open }" data-slot="collapsible" v-bind="forwarded">
    <slot :open="open" />
  </CollapsibleRoot>
</template>
