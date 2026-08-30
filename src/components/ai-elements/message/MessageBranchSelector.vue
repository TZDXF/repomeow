<script setup lang="ts">
import { ButtonGroup } from "@/components/ui/button-group";
import { useMessageBranchContext } from "./context";
import type { MessageRole } from "./types";

interface Props {
  from: MessageRole;
}
defineProps<Props>();

const { totalBranches } = useMessageBranchContext();
</script>

<template>
  <ButtonGroup
    v-if="totalBranches > 1"
    class="[&>*:not(:first-child)]:rounded-l-md [&>*:not(:last-child)]:rounded-r-md"
    orientation="horizontal"
    v-bind="$attrs"
  >
    <slot />
  </ButtonGroup>
</template>
