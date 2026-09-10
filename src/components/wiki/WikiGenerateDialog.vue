<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";

import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { Project } from "@/types";
import { loadWikiConfig, saveWikiConfig } from "@/lib/wiki";
import AgentModelThinkingSelect from "@/components/ai-elements/AgentModelThinkingSelect.vue";
import { useAiConfigStore } from "@/stores/ai-config";
import type { WikiGenerationConfig } from "@/lib/wiki-generator";

/**
 * Wiki 生成配置对话框:点「生成/重新生成」(generate 模式)或 wiki 页右上角
 * 配置入口(edit 模式)时打开。生成始终使用内置 Agent,可按厂商列出 ai-config
 * 全部模型并选思考强度/并发数(空 = 设置页默认模型与全局并发)。打开时读取
 * 当前项目 Wiki 目录的 config.json,确认才写回该项目;取消则丢弃改动。
 * 是否随之触发生成由调用方决定。
 */
const props = defineProps<{
  /** false 表示关闭 */
  open: boolean;
  /** 当前项目路径，用于读写其独立 Wiki 生成配置。 */
  projectPath: string;
  /** generate = 确认按钮为「开始生成」;edit = 仅保存配置,按钮为「保存」 */
  mode?: "generate" | "edit";
}>();
const emit = defineEmits<{ close: []; confirm: [] }>();

const { t } = useI18n();

/** ai-config 配置副本:模型清单来源(空/未加载时选择器禁用走默认) */
const aiConfig = useAiConfigStore();
const projects = useProjectsStore();
const settings = useSettingsStore();
const projectId = ref<number | null>(null);
const wikiAutoUpdate = ref(false);
const initialWikiAutoUpdate = ref(false);

const open = computed({
  get: () => props.open,
  set: (v: boolean) => {
    if (!v) {
      emit("close");
    }
  },
});

// ── 本地副本:打开时从项目 config.json 同步,确认才写回 ────────────────────

const model = ref("");
const thinking = ref("");
/** 页面并发数(1-8;空 = 设置页全局 AI 并发) */
const concurrency = ref<number | "">("");
const configLoading = ref(false);
const configSaving = ref(false);
const configError = ref("");
let loadSequence = 0;

watch(
  () => [props.open, props.projectPath] as const,
  ([isOpen]) => {
    if (!isOpen) {
      return;
    }
    void loadProjectConfig();
  },
  { immediate: true },
);

async function loadProjectConfig() {
  const sequence = ++loadSequence;
  configLoading.value = true;
  configError.value = "";
  model.value = "";
  thinking.value = "";
  concurrency.value = "";
  projectId.value = null;
  wikiAutoUpdate.value = false;
  initialWikiAutoUpdate.value = false;
  const projectPath = props.projectPath;
  try {
    const [config, project] = await Promise.all([
      loadWikiConfig(projectPath),
      cmd<Project | null>("get_project_by_path", { path: projectPath }),
    ]);
    if (sequence !== loadSequence || !props.open) return;
    projectId.value = project?.id ?? null;
    wikiAutoUpdate.value = project?.wiki_auto_update ?? false;
    initialWikiAutoUpdate.value = wikiAutoUpdate.value;
    model.value = config.model ?? "";
    thinking.value = config.thinking ?? "";
    concurrency.value = config.concurrency ? Math.min(8, Math.max(1, config.concurrency)) : "";
  } catch (error) {
    if (sequence === loadSequence) {
      configError.value = error instanceof Error ? error.message : String(error);
    }
  } finally {
    if (sequence === loadSequence) configLoading.value = false;
  }
}

/** 已选模型引用是否仍指向现存厂商与模型 */
function builtinModelExists(value: string): boolean {
  const separator = value.indexOf("/");
  if (separator <= 0) return false;
  const provider = aiConfig.config?.providers[value.slice(0, separator)];
  return Boolean(provider?.models.some((m) => m.id === value.slice(separator + 1)));
}

// ── 提交 ────────────────────────────────────────────────────────────────────

/** edit 模式仅保存配置(右上角入口),generate 模式保存后由调用方触发生成 */
const confirmLabel = computed(() =>
  props.mode === "edit" ? t("common.save") : t("wiki.genConfirm"),
);

async function confirm() {
  if (configLoading.value || configSaving.value) return;
  const projectPath = props.projectPath;
  const id = projectId.value;
  const autoUpdate = wikiAutoUpdate.value;
  const shouldSaveAutoUpdate =
    !settings.wikiAutoUpdate && autoUpdate !== initialWikiAutoUpdate.value;
  const config: WikiGenerationConfig = {
    version: 1,
    // 失效引用(厂商/模型已从 ai-config 删除)不落盘,回退设置页默认模型
    model: model.value && builtinModelExists(model.value) ? model.value : undefined,
    thinking: thinking.value || undefined,
    concurrency:
      typeof concurrency.value === "number" && concurrency.value > 0
        ? Math.min(8, Math.max(1, Math.round(concurrency.value)))
        : undefined,
  };
  configSaving.value = true;
  configError.value = "";
  try {
    await saveWikiConfig(projectPath, config);
    if (id !== null && shouldSaveAutoUpdate) {
      await projects.setWikiAutoUpdate(id, autoUpdate);
      initialWikiAutoUpdate.value = autoUpdate;
    }
    if (props.open && props.projectPath === projectPath) emit("confirm");
  } catch (error) {
    configError.value = error instanceof Error ? error.message : String(error);
  } finally {
    configSaving.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[85vh] flex-col sm:max-w-lg">
      <DialogHeader class="shrink-0">
        <DialogTitle>{{ t("wiki.genConfigTitle") }}</DialogTitle>
        <p class="mt-1 text-xs text-muted-foreground">{{ t("wiki.genConfigDesc") }}</p>
      </DialogHeader>

      <ScrollArea class="min-h-0 flex-1 py-2">
        <!-- 模型(按厂商列 ai-config 全部模型)/ 思考强度 / 并发数 -->
        <div class="grid gap-3">
          <AgentModelThinkingSelect
            v-model:model="model"
            v-model:thinking="thinking"
            :disabled="configLoading || configSaving"
          />
          <div class="flex min-w-0 flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("wiki.agentConcurrency") }}</label>
            <Input
              v-model.number="concurrency"
              type="number"
              min="1"
              max="8"
              :placeholder="t('wiki.builtinConcurrencyPlaceholder')"
              :disabled="configLoading || configSaving"
            />
            <p class="text-xs text-muted-foreground">{{ t("wiki.builtinConcurrencyHint") }}</p>
          </div>
        </div>
      </ScrollArea>

      <div class="flex items-center justify-between gap-3 rounded-md border px-3 py-2.5">
        <div class="min-w-0 flex-1">
          <label for="wiki-generation-auto-update" class="text-sm font-medium">
            {{ t("settings.tracking.wikiAutoUpdateLabel") }}
          </label>
          <p class="mt-0.5 text-xs text-muted-foreground">
            {{
              settings.wikiAutoUpdate
                ? t("settings.tracking.wikiToggleGloballyOn")
                : t("settings.tracking.wikiToggleHint")
            }}
          </p>
        </div>
        <Switch
          id="wiki-generation-auto-update"
          class="shrink-0"
          :model-value="settings.wikiAutoUpdate || wikiAutoUpdate"
          :disabled="configLoading || configSaving || settings.wikiAutoUpdate || projectId === null"
          @update:model-value="wikiAutoUpdate = $event"
        />
      </div>

      <p v-if="configError" class="text-xs text-destructive">
        {{ t("wiki.genConfigError", { error: configError }) }}
      </p>

      <div class="flex shrink-0 justify-end gap-2 pt-2">
        <Button variant="outline" size="sm" :disabled="configSaving" @click="emit('close')">
          {{ t("common.cancel") }}
        </Button>
        <Button size="sm" :disabled="configLoading || configSaving" @click="confirm">
          <Loader2 v-if="configLoading || configSaving" class="h-4 w-4 animate-spin" />
          {{ confirmLabel }}
        </Button>
      </div>
    </DialogContent>
  </Dialog>
</template>
