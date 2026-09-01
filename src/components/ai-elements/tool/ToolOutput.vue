<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { computed, isVNode } from "vue";

// 改造说明:原实现用 CodeBlock 展示结果(JSON 高亮 + 标题栏),短结果也占一个大块。
// 改为限高纯文本块(内部滚动、保留换行),错误态红底;结果多为自然语言摘要,
// 不再做语法高亮。
interface Props extends /* @vue-ignore */ HTMLAttributes {
  output?: unknown;
  errorText?: string;
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();

const showOutput = computed(
  () => (props.output !== undefined && props.output !== null) || props.errorText,
);

const text = computed(() => {
  if (props.errorText) {
    return props.errorText;
  }
  if (typeof props.output === "string") {
    return props.output;
  }
  if (typeof props.output === "object" && props.output !== null && !isVNode(props.output)) {
    return JSON.stringify(props.output, null, 2);
  }
  return String(props.output ?? "");
});
</script>

<template>
  <div v-if="showOutput" :class="cn('px-2 pt-0.5 pb-1.5', props.class)" v-bind="$attrs">
    <div
      :class="
        cn(
          'max-h-48 overflow-y-auto rounded-md px-2 py-1.5 text-xs',
          errorText ? 'bg-destructive/10 text-destructive' : 'bg-muted/50 text-foreground/80',
        )
      "
    >
      <span class="whitespace-pre-wrap break-words">{{ text }}</span>
    </div>
  </div>
</template>
