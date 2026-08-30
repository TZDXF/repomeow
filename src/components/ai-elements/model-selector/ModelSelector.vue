<script setup lang="ts">
import { computed } from "vue";
import { Sparkles } from "@lucide/vue";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { modelDisplayName, type ModelSelectorGroup } from "./types";

/**
 * 模型选择器(按厂商分组):value 为复合值 "providerId/modelId"。
 * 支持思考的模型以 ✦ 徽标标注。选项为空时整体禁用。
 */
const props = withDefaults(
  defineProps<{
    modelValue: string;
    groups: ModelSelectorGroup[];
    placeholder?: string;
    disabled?: boolean;
    size?: "sm" | "default";
    /** 覆盖触发器的宽度约束(默认 min-w-0 max-w-44,浮层紧凑场景用) */
    triggerClass?: string;
  }>(),
  { placeholder: "", disabled: false, size: "sm", triggerClass: "min-w-0 max-w-44" },
);

const emit = defineEmits<{ "update:modelValue": [value: string] }>();

const hasAnyModel = computed(() => props.groups.some((group) => group.models.length > 0));
</script>

<template>
  <Select
    :model-value="modelValue"
    :disabled="disabled || !hasAnyModel"
    @update:model-value="emit('update:modelValue', String($event))"
  >
    <SelectTrigger :size="size" :class="triggerClass">
      <SelectValue :placeholder="placeholder" />
    </SelectTrigger>
    <SelectContent class="max-h-80">
      <template v-for="group in groups" :key="group.providerId">
        <SelectGroup v-if="group.models.length">
          <SelectLabel>{{ group.providerName }}</SelectLabel>
          <SelectItem
            v-for="model in group.models"
            :key="model.id"
            :value="`${group.providerId}/${model.id}`"
          >
            <span class="flex items-center gap-1.5 overflow-hidden">
              <span class="truncate">{{ modelDisplayName(model) }}</span>
              <Sparkles v-if="model.reasoning" class="size-3 shrink-0 text-muted-foreground" />
            </span>
          </SelectItem>
        </SelectGroup>
      </template>
    </SelectContent>
  </Select>
</template>
