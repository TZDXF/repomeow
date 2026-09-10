<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import AgentModelThinkingSelect from "@/components/ai-elements/AgentModelThinkingSelect.vue";

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
    // 每次打开回到默认选项
    model.value = "";
    thinking.value = "";
  },
  { immediate: true },
);

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
        <AgentModelThinkingSelect v-model:model="model" v-model:thinking="thinking" />
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
