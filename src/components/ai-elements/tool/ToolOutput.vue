<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { computed, isVNode } from "vue";
import { useI18n } from "vue-i18n";
import { CodeBlock } from "../code-block";

interface Props extends /* @vue-ignore */ HTMLAttributes {
  output?: unknown;
  errorText?: string;
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();

const { t } = useI18n();

const showOutput = computed(
  () => (props.output !== undefined && props.output !== null) || props.errorText,
);

const isObjectOutput = computed(
  () => typeof props.output === "object" && props.output !== null && !isVNode(props.output),
);
const isStringOutput = computed(() => typeof props.output === "string");

const formattedOutput = computed(() => {
  if (isObjectOutput.value) {
    return JSON.stringify(props.output, null, 2);
  }
  return props.output as string;
});
</script>

<template>
  <div v-if="showOutput" :class="cn('space-y-1.5 p-2', props.class)" v-bind="$attrs">
    <h4 class="font-medium text-muted-foreground text-xs uppercase tracking-wide">
      {{ props.errorText ? t("chat.toolError") : t("chat.toolOutput") }}
    </h4>
    <div
      :class="
        cn(
          'overflow-x-auto rounded-md text-xs [&_table]:w-full',
          props.errorText ? 'bg-destructive/10 text-destructive' : 'bg-muted/50 text-foreground',
        )
      "
    >
      <!-- Error text -->
      <div v-if="errorText">
        {{ props.errorText }}
      </div>

      <!-- Output rendering based on type -->
      <CodeBlock v-if="isObjectOutput" :code="formattedOutput" language="json" />
      <CodeBlock v-else-if="isStringOutput" :code="formattedOutput" language="json" />
      <div v-else-if="output !== undefined && output !== null">
        {{ props.output }}
      </div>
    </div>
  </div>
</template>
