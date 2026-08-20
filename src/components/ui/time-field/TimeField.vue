<script setup lang="ts">
import { Time } from "@internationalized/date";
import { TimeFieldInput, TimeFieldRoot } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { computed } from "vue";
import { cn } from "@/lib/utils";

const props = withDefaults(
  defineProps<{
    /** "HH:MM" 字符串(24 小时制) */
    modelValue: string;
    class?: HTMLAttributes["class"];
    disabled?: boolean;
  }>(),
  { disabled: false },
);
const emit = defineEmits<{ "update:modelValue": [value: string] }>();

const timeValue = computed<Time | undefined>({
  get: () => {
    const [h, m] = props.modelValue.split(":").map((x) => Number.parseInt(x, 10));
    if (Number.isNaN(h) || Number.isNaN(m)) return undefined;
    return new Time(h, m);
  },
  set: (v) => {
    if (!v) return;
    emit(
      "update:modelValue",
      `${String(v.hour).padStart(2, "0")}:${String(v.minute).padStart(2, "0")}`,
    );
  },
});
</script>

<template>
  <TimeFieldRoot
    v-model="timeValue"
    v-slot="{ segments }"
    data-slot="time-field"
    :hour-cycle="24"
    :disabled="disabled"
    :class="
      cn(
        'inline-flex h-8 items-center rounded-md border border-input bg-transparent px-2 text-sm shadow-xs outline-none focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50',
        props.class,
      )
    "
  >
    <template v-for="item in segments" :key="item.part">
      <TimeFieldInput v-if="item.part === 'literal'" :part="item.part" class="text-muted-foreground">
        {{ item.value }}
      </TimeFieldInput>
      <TimeFieldInput
        v-else
        :part="item.part"
        class="w-7 rounded-sm px-0.5 text-center tabular-nums outline-none focus:bg-accent data-[placeholder]:text-muted-foreground"
      >
        {{ item.value }}
      </TimeFieldInput>
    </template>
  </TimeFieldRoot>
</template>
