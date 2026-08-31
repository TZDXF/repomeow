<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { CodeBlock } from "../code-block";

interface Props extends /* @vue-ignore */ HTMLAttributes {
  input: unknown;
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();

const { t } = useI18n();

const formattedInput = computed(() => {
  return JSON.stringify(props.input, null, 2);
});
</script>

<template>
  <div :class="cn('space-y-1.5 overflow-hidden p-2', props.class)" v-bind="$attrs">
    <h4 class="font-medium text-muted-foreground text-xs uppercase tracking-wide">
      {{ t("chat.toolInput") }}
    </h4>
    <div class="rounded-md bg-muted/50">
      <CodeBlock :code="formattedInput" language="json" />
    </div>
  </div>
</template>
