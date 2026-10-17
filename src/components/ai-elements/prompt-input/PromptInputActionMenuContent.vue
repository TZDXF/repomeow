<script setup lang="ts">
import { DropdownMenuContent } from "@/components/common/project-floating-content";
import type { HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

type DropdownMenuContentProps = InstanceType<typeof DropdownMenuContent>["$props"];

interface Props extends /* @vue-ignore */ DropdownMenuContentProps {
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();

const { align, class: _, ...restProps } = props;
</script>

<template>
  <DropdownMenuContent align="start" :class="cn(props.class)" v-bind="restProps">
    <slot />
  </DropdownMenuContent>
</template>
