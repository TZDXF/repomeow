<script setup lang="ts">
import { SelectContent } from "@/components/common/project-floating-content";
import type { HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

type SelectContentProps = InstanceType<typeof SelectContent>["$props"];

interface Props extends /* @vue-ignore */ SelectContentProps {
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();
</script>

<template>
  <SelectContent :class="cn(props.class)" v-bind="props">
    <slot />
  </SelectContent>
</template>
