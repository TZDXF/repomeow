<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  ModelSelector,
  modelDisplayName,
  parseModelOptionValue,
  type ModelSelectorGroup,
} from "@/components/ai-elements/model-selector";
import { CHAT_THINKING_LEVELS } from "@/lib/ai-config";
import { useAiConfigStore } from "@/stores/ai-config";

/**
 * 内置 Agent 任务的「模型 + 思考强度」组合选择器:
 * 空值 = 设置页默认模型 / 模型默认档(推理模型中档,其余关闭),
 * 触发器直接展示生效的默认名并标注「默认」,下拉项不再重复标注。
 * 两个字段各自带 label,父级用 grid/flex 控制布局。
 */
withDefaults(
  defineProps<{
    disabled?: boolean;
    /** 触发器尺寸 */
    size?: "sm" | "default";
    /** 覆盖两个触发器的宽度约束 */
    triggerClass?: string;
  }>(),
  {
    disabled: false,
    size: "default",
    triggerClass: "min-w-0 w-full",
  },
);

/** 模型复合值("providerId/modelId");空 = 设置页默认模型 */
const model = defineModel<string>("model", { default: "" });
/** 思考强度;空 = 模型默认档 */
const thinking = defineModel<string>("thinking", { default: "" });

const { t } = useI18n();
const aiConfig = useAiConfigStore();

onMounted(() => {
  // 模型清单加载失败不阻塞:选择器空态禁用走默认
  aiConfig.ensureLoaded().catch(() => {});
});

/** 空值的 Select 占位值(reka 不接受空字符串作为 item value) */
const DEFAULT_VALUE = "__default__";

/** 可选模型:ai-config 全部厂商/模型,按厂商分组 */
const modelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) return [];
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
});

/** 模型触发器占位:展示设置页默认模型名并标注「默认」,无默认模型时回退通用文案 */
const modelPlaceholder = computed(() => {
  const resolved = aiConfig.defaultModel;
  if (!resolved) return t("wiki.builtinModelDefault");
  return `${modelDisplayName(resolved.model)} (${t("chat.defaultTag")})`;
});

/** 生效模型定义:已选 > 设置页默认(决定思考默认档:推理模型中档,其余关闭) */
const effectiveModelDef = computed(() => {
  const config = aiConfig.config;
  if (model.value && config) {
    const reference = parseModelOptionValue(model.value);
    const found = reference
      ? config.providers[reference.providerId]?.models.find((m) => m.id === reference.modelId)
      : undefined;
    if (found) return found;
  }
  return aiConfig.defaultModel?.model ?? null;
});

/** 思考默认项文案:直接展示生效档位(推理模型「中」,其余「关闭」)并标注「默认」 */
const thinkingDefaultLabel = computed(() => {
  const def = effectiveModelDef.value;
  if (!def) return t("wiki.builtinThinkingDefault");
  const level = def.reasoning ? "medium" : "off";
  return `${t(`chat.thinkingLevels.${level}`)} (${t("chat.defaultTag")})`;
});

const thinkingOptions = computed(() =>
  CHAT_THINKING_LEVELS.map((level) => ({
    id: level,
    name: t(`chat.thinkingLevels.${level}`),
  })),
);

function onModelChange(value: string) {
  model.value = value;
}

function onThinkingChange(value: unknown) {
  if (typeof value === "string") {
    thinking.value = value === DEFAULT_VALUE ? "" : value;
  }
}
</script>

<template>
  <div class="flex min-w-0 flex-col gap-1.5">
    <label class="text-sm font-medium">{{ t("wiki.agentModel") }}</label>
    <ModelSelector
      :model-value="model"
      :groups="modelGroups"
      :placeholder="modelPlaceholder"
      :disabled="disabled"
      :size="size"
      :trigger-class="triggerClass"
      hide-default-tag
      @update:model-value="onModelChange"
    />
  </div>
  <div class="flex min-w-0 flex-col gap-1.5">
    <label class="text-sm font-medium">{{ t("wiki.agentThinking") }}</label>
    <Select
      :model-value="thinking || DEFAULT_VALUE"
      :disabled="disabled"
      @update:model-value="onThinkingChange"
    >
      <SelectTrigger :class="triggerClass">
        <SelectValue class="min-w-0 flex-1 truncate text-left" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem :value="DEFAULT_VALUE">{{ thinkingDefaultLabel }}</SelectItem>
        <SelectItem v-for="c in thinkingOptions" :key="c.id" :value="c.id">
          {{ c.name }}
        </SelectItem>
      </SelectContent>
    </Select>
  </div>
</template>
