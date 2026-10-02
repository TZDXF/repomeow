<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { Brain, Eye, Wrench } from "@lucide/vue";
import { ModelSelector, type ModelSelectorGroup } from "@/components/ai-elements/model-selector";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import { CHAT_THINKING_LEVELS, type ChatPermission, type ChatThinkingLevel } from "@/lib/ai-config";
import { useAiConfigStore } from "@/stores/ai-config";

// ── 底栏偏好:模型 / 思考强度 / 权限 ──────────────────
// 偏好写入 ai-config.json 后 Rust 侧每次 chat_send 前热读;回答回合进行中
// 一律禁用切换,避免在途 LLM 调用与界面状态不一致。
const props = defineProps<{
  busy: boolean;
  aiReady: boolean;
}>();
const { t } = useI18n();
const router = useRouter();
const aiConfig = useAiConfigStore();

const modelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) return [];
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
});

const thinkingDisabled = computed(() => !aiConfig.chatModel?.model.reasoning);
const thinkingTitle = computed(() =>
  thinkingDisabled.value ? t("chat.thinkingUnsupported") : t("chat.thinking"),
);

const permissionTitle = computed(() => t(`chat.permission.${aiConfig.chatPermission}`));

/** 偏好写入失败统一 toast(落盘失败时 store 已回读后端真实状态) */
function applyPref(action: () => Promise<void>) {
  return action().catch((error: unknown) => {
    toast.error(error instanceof Error ? error.message : String(error));
  });
}

function onModelChange(value: string) {
  void applyPref(() => aiConfig.setChatModelValue(value));
}

function onThinkingChange(level: unknown) {
  if (typeof level !== "string") return;
  void applyPref(() => aiConfig.setChatThinking(level as ChatThinkingLevel));
}

function onPermissionChange(value: unknown) {
  if (value !== "all" && value !== "ask") return;
  void applyPref(() => aiConfig.setChatPermission(value as ChatPermission));
}
</script>

<template>
  <!-- body-lock 必须与 disable-outside-pointer-events=false 成对出现:
       reka Select 的 bodyLock 默认 true 会给 body 写 pointer-events:none,
       内容不自恢复 auto 时选项无法点击(见 ModelSelector.vue 注释) -->
  <Select
    :model-value="aiConfig.chatPermission"
    :disabled="props.busy"
    @update:model-value="onPermissionChange"
  >
    <SelectTrigger
      size="sm"
      class="text-muted-foreground h-7 gap-1 px-2 text-xs"
      :title="permissionTitle"
    >
      <Eye v-if="aiConfig.chatPermission !== 'all'" class="size-3.5 shrink-0" />
      <Wrench v-else class="size-3.5 shrink-0" />
      <span>
        {{ t(`chat.permission.${aiConfig.chatPermission}Short`) }}
      </span>
    </SelectTrigger>
    <SelectContent :disable-outside-pointer-events="false" :body-lock="false">
      <SelectItem value="readOnly" class="text-xs" :title="t('chat.permission.readOnly')">
        {{ t("chat.permission.readOnlyShort") }}
      </SelectItem>
      <SelectItem value="ask" class="text-xs" :title="t('chat.permission.ask')">
        {{ t("chat.permission.askShort") }}
      </SelectItem>
      <SelectItem value="all" class="text-xs" :title="t('chat.permission.all')">
        {{ t("chat.permission.allShort") }}
      </SelectItem>
    </SelectContent>
  </Select>
  <ModelSelector
    :model-value="aiConfig.chatModelValue"
    :groups="modelGroups"
    :disabled="!props.aiReady || props.busy"
    :placeholder="t('chat.modelPlaceholder')"
    @update:model-value="onModelChange"
  />
  <Select
    :model-value="aiConfig.chatThinking"
    :disabled="!props.aiReady || thinkingDisabled || props.busy"
    @update:model-value="onThinkingChange"
  >
    <SelectTrigger
      size="sm"
      class="text-muted-foreground h-7 gap-1 px-2 text-xs"
      :title="thinkingTitle"
    >
      <Brain class="size-3.5 shrink-0" />
      <SelectValue />
    </SelectTrigger>
    <SelectContent :disable-outside-pointer-events="false" :body-lock="false">
      <SelectItem v-for="level in CHAT_THINKING_LEVELS" :key="level" :value="level" class="text-xs">
        {{ t(`chat.thinkingLevels.${level}`)
        }}{{ level === "medium" ? ` (${t("chat.defaultTag")})` : "" }}
      </SelectItem>
    </SelectContent>
  </Select>
  <p v-if="!props.aiReady" class="text-muted-foreground w-full text-xs">
    {{ t("chat.notConfigured") }}
    <Button variant="link" size="sm" class="h-auto p-0 text-xs" @click="router.push('/settings')">
      {{ t("chat.goSettings") }}
    </Button>
  </p>
</template>
