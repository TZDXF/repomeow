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
    <!-- disableOutsidePointerEvents=false:浮层场景(如 ChatDock 面板 pointer-events-auto)
         下 reka 默认的 body pointer-events:none 拦截不到面板内其他触发器,
         会导致两个下拉同时打开;关闭后点另一个下拉即可正常收起到前一个。
         必须同时 bodyLock=false:reka Select 的 bodyLock 默认 true,useBodyScrollLock
         会无条件给 body 写 pointer-events:none,disableOutsidePointerEvents=false 时
         内容不再自恢复 auto,选项会整体失去点击命中(下拉展开但无法选择) -->
    <SelectContent class="max-h-80" :disable-outside-pointer-events="false" :body-lock="false">
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
