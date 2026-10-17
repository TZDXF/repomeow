<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { InputGroupAddon } from "@/components/ui/input-group";
import { cn } from "@/lib/utils";

type PromptInputFooterProps = InstanceType<typeof InputGroupAddon>["$props"];

interface Props extends /* @vue-ignore */ Omit<PromptInputFooterProps, "align"> {
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();
</script>

<template>
  <InputGroupAddon
    v-bind="props"
    align="block-end"
    :class="cn('min-w-0 flex-nowrap justify-between gap-1', props.class)"
  >
    <slot />
  </InputGroupAddon>
</template>
