<script setup lang="ts">
import type { CollapsibleTriggerProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { reactiveOmit } from "@vueuse/core";
import { CollapsibleTrigger } from "reka-ui";

const props = defineProps<CollapsibleTriggerProps & { class?: HTMLAttributes["class"] }>(),
  delegatedProps = reactiveOmit(props, "class");
</script>

<template>
  <CollapsibleTrigger data-slot="collapsible-trigger" v-bind="delegatedProps" :class="props.class">
    <slot />
  </CollapsibleTrigger>
</template>
