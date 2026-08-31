<script setup lang="ts">
import type { HTMLAttributes } from "vue";
import { Collapsible } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { useVModel } from "@vueuse/core";
import { computed, provide, ref, watch } from "vue";
import { ReasoningKey } from "./context";

// 移植自 ai-elements-vue reasoning:思考过程折叠面板。
// 流式开始自动展开,流式结束 AUTO_CLOSE_DELAY 后自动收起(仅一次,
// 用户手动展开不再干预);未传 duration 时自行计时。
interface Props {
  class?: HTMLAttributes["class"];
  isStreaming?: boolean;
  open?: boolean;
  defaultOpen?: boolean;
  /** 思考耗时(秒);缺省时组件按 isStreaming 起止自行计时 */
  duration?: number;
}

const props = withDefaults(defineProps<Props>(), {
  isStreaming: false,
  defaultOpen: true,
  duration: undefined,
});

const emit = defineEmits<{
  (e: "update:open", value: boolean): void;
  (e: "update:duration", value: number): void;
}>();

const isOpen = useVModel(props, "open", emit, {
  defaultValue: props.defaultOpen,
  passive: true,
});

const internalDuration = ref<number | undefined>(props.duration);

watch(
  () => props.duration,
  (newVal) => {
    internalDuration.value = newVal;
  },
);

function updateDuration(val: number) {
  internalDuration.value = val;
  emit("update:duration", val);
}

const hasAutoClosed = ref(false);
const startTime = ref<number | null>(null);

const MS_IN_S = 1000;
const AUTO_CLOSE_DELAY = 1000;

// 随流式起止记录耗时
watch(
  () => props.isStreaming,
  (streaming) => {
    if (streaming) {
      isOpen.value = true;
      if (startTime.value === null && props.duration === undefined) {
        startTime.value = Date.now();
      }
    } else if (startTime.value !== null) {
      const calculatedDuration = Math.ceil((Date.now() - startTime.value) / MS_IN_S);
      updateDuration(calculatedDuration);
      startTime.value = null;
    }
  },
  { immediate: true },
);

// 流式结束后自动收起(仅一次)
watch(
  [() => props.isStreaming, isOpen, () => props.defaultOpen, hasAutoClosed],
  (_, __, onCleanup) => {
    if (props.defaultOpen && !props.isStreaming && isOpen.value && !hasAutoClosed.value) {
      const timer = setTimeout(() => {
        isOpen.value = false;
        hasAutoClosed.value = true;
      }, AUTO_CLOSE_DELAY);
      onCleanup(() => clearTimeout(timer));
    }
  },
  { immediate: true },
);

provide(ReasoningKey, {
  isStreaming: computed(() => props.isStreaming),
  isOpen,
  setIsOpen: (val: boolean) => {
    isOpen.value = val;
  },
  duration: computed(() => internalDuration.value),
});
</script>

<template>
  <Collapsible v-model:open="isOpen" :class="cn('not-prose mb-2', props.class)">
    <slot />
  </Collapsible>
</template>
