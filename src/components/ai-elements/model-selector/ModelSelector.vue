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
import { useI18n } from "vue-i18n";
import { useAiConfigStore } from "@/stores/ai-config";
import { modelDisplayName, type ModelSelectorGroup } from "./types";

/**
 * 模型选择器(按厂商分组):value 为复合值 "providerId/modelId"。
 * 支持思考的模型以 ✦ 徽标标注。选项为空时整体禁用;
 * `genericOption` 可在所有分组前附加一个通用选项(如「默认模型」)。
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
    /** 附加在所有分组之前的通用选项(value 不得含 "/",避免与复合值混淆) */
    genericOption?: { value: string; label: string };
    /** 隐藏下拉项的「默认」徽标(触发器已展示默认模型名的场景用) */
    hideDefaultTag?: boolean;
  }>(),
  {
    placeholder: "",
    disabled: false,
    size: "sm",
    triggerClass: "min-w-0 max-w-44",
    genericOption: undefined,
    hideDefaultTag: false,
  },
);

const emit = defineEmits<{ "update:modelValue": [value: string] }>();

const { t } = useI18n();
const aiConfig = useAiConfigStore();

/** 是否为全局默认模型(AI 设置中的「默认模型」) */
function isDefaultModel(providerId: string, modelId: string): boolean {
  const reference = aiConfig.defaultModel?.reference;
  return reference?.providerId === providerId && reference?.modelId === modelId;
}

const hasAnyOption = computed(
  () => Boolean(props.genericOption) || props.groups.some((group) => group.models.length > 0),
);
</script>

<template>
  <Select
    :model-value="modelValue"
    :disabled="disabled || !hasAnyOption"
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
      <SelectItem v-if="genericOption" :value="genericOption.value" class="text-muted-foreground">
        {{ genericOption.label }}
      </SelectItem>
      <template v-for="group in groups" :key="group.providerId">
        <SelectGroup v-if="group.models.length">
          <SelectLabel>{{ group.providerName }}</SelectLabel>
          <SelectItem
            v-for="model in group.models"
            :key="model.id"
            :value="`${group.providerId}/${model.id}`"
          >
            <span class="flex items-center gap-1.5 overflow-hidden">
              <span class="truncate"
                >{{ modelDisplayName(model)
                }}{{
                  !hideDefaultTag && isDefaultModel(group.providerId, model.id)
                    ? ` (${t("chat.defaultTag")})`
                    : ""
                }}</span
              >
              <Sparkles v-if="model.reasoning" class="size-3 shrink-0 text-muted-foreground" />
            </span>
          </SelectItem>
        </SelectGroup>
      </template>
    </SelectContent>
  </Select>
</template>
