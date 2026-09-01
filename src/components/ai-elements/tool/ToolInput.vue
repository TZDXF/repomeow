<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import { computed } from "vue";

// 改造说明:原实现用 CodeBlock 展示整个 JSON(带标题栏/高亮 chrome),
// 参数区视觉面积过大。改为摊平成 key/value 行:键用柔和色、值等宽单行截断,
// 完整值经 title 悬停可见;非对象参数兜底单行 JSON。
interface Props extends /* @vue-ignore */ HTMLAttributes {
  input: unknown;
  class?: HTMLAttributes["class"];
}

const props = defineProps<Props>();

function formatValue(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  return JSON.stringify(value);
}

const entries = computed<[string, string][]>(() => {
  if (props.input !== null && typeof props.input === "object" && !Array.isArray(props.input)) {
    return Object.entries(props.input as Record<string, unknown>).map(([key, value]) => [
      key,
      formatValue(value),
    ]);
  }
  return [];
});

const fallback = computed(() => formatValue(props.input));
</script>

<template>
  <div :class="cn('space-y-0.5 px-2 pt-0.5 pb-1', props.class)" v-bind="$attrs">
    <template v-if="entries.length">
      <div v-for="[key, value] in entries" :key="key" class="flex items-baseline gap-2 text-xs">
        <span class="shrink-0 text-muted-foreground">{{ key }}</span>
        <span class="min-w-0 truncate font-mono text-foreground/80" :title="value">
          {{ value }}
        </span>
      </div>
    </template>
    <div v-else class="truncate font-mono text-foreground/80 text-xs" :title="fallback">
      {{ fallback }}
    </div>
  </div>
</template>
