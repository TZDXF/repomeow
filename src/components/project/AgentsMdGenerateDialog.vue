<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ModelSelector, type ModelSelectorGroup } from "@/components/ai-elements/model-selector";
import { CHAT_THINKING_LEVELS } from "@/lib/ai-config";
import { useAiConfigStore } from "@/stores/ai-config";

/**
 * 生成 AGENTS.md 配置对话框(内置 Agent 后端):可选模型(空 = 设置页默认模型)
 * 与思考强度(空 = 模型默认档:推理模型中档,其余关闭)。确认时把选择回传给调用方,
 * 是否触发生成由调用方决定;取消则丢弃改动。
 */
const props = defineProps<{
  /** false 表示关闭 */
  open: boolean;
  /** 已存在 AGENTS.md:标题/提示切换为重新生成语义 */
  regenerate: boolean;
}>();
const emit = defineEmits<{
  close: [];
  confirm: [options: { model?: string; thinking?: string }];
}>();

const { t } = useI18n();
const aiConfig = useAiConfigStore();

const DEFAULT_VALUE = "__default__";
const model = ref("");
const thinking = ref("");

const open = computed({
  get: () => props.open,
  set: (v: boolean) => {
    if (!v) {
      emit("close");
    }
  },
});

watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) {
      return;
    }
    // 每次打开回到默认选项;模型清单加载失败不阻塞(选择器空态禁用走默认)
    model.value = "";
    thinking.value = "";
    aiConfig.ensureLoaded().catch(() => {});
  },
  { immediate: true },
);

/** 可选模型:ai-config 全部厂商/模型,按厂商分组(与 Wiki 生成对话框一致) */
const modelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) {
    return [];
  }
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
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

function confirm() {
  emit("confirm", {
    model: model.value || undefined,
    thinking: thinking.value || undefined,
  });
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{{
          regenerate ? t("aiAssets.regenerateAgentsMd") : t("aiAssets.generateAgentsMd")
        }}</DialogTitle>
      </DialogHeader>
      <div class="grid gap-3 sm:grid-cols-2">
        <div class="flex min-w-0 flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("wiki.agentModel") }}</label>
          <ModelSelector
            :model-value="model"
            :groups="modelGroups"
            :placeholder="t('wiki.builtinModelDefault')"
            size="default"
            trigger-class="min-w-0 w-full"
            @update:model-value="onModelChange"
          />
        </div>
        <div class="flex min-w-0 flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("wiki.agentThinking") }}</label>
          <Select :model-value="thinking || DEFAULT_VALUE" @update:model-value="onThinkingChange">
            <SelectTrigger class="min-w-0 w-full">
              <SelectValue class="min-w-0 flex-1 truncate text-left" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem :value="DEFAULT_VALUE">{{ t("wiki.builtinThinkingDefault") }}</SelectItem>
              <SelectItem v-for="c in thinkingOptions" :key="c.id" :value="c.id">
                {{ c.name }}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
      <p v-if="regenerate" class="text-xs text-muted-foreground">
        {{ t("aiAssets.agentsMdOverwriteHint") }}
      </p>
      <div class="flex justify-end gap-2 pt-2">
        <Button variant="outline" size="sm" @click="emit('close')">{{ t("common.cancel") }}</Button>
        <Button size="sm" @click="confirm">{{ t("wiki.genConfirm") }}</Button>
      </div>
    </DialogContent>
  </Dialog>
</template>
