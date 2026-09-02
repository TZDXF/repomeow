<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { acpTestCached, agentList, type AcpTestResult } from "@/lib/agent";
import { CHAT_THINKING_LEVELS } from "@/lib/ai-config";
import { loadWikiConfig, saveWikiConfig } from "@/lib/wiki";
import { ModelSelector, type ModelSelectorGroup } from "@/components/ai-elements/model-selector";
import { useAiConfigStore } from "@/stores/ai-config";
import type { WikiGenBackend, WikiGenerationConfig } from "@/lib/wiki-generator";

/**
 * Wiki 生成配置对话框:点「生成/重新生成」(generate 模式)或 wiki 页右上角
 * 配置入口(edit 模式)时打开,选择后端(内置 Agent / 已安装的精选 agent)。
 * 内置 Agent 可按厂商列出 ai-config 全部模型并选思考强度/并发数(空 = 设置页
 * 默认模型与全局并发);agent 后端探测其上报的模型/思考强度清单(acpTestCached
 * 应用会话级缓存,不重复 spawn)。打开时读取当前项目 Wiki 目录的 config.json,
 * 确认才写回该项目;取消则丢弃改动。是否随之触发生成由调用方决定。
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

/** ai-config 配置副本:内置后端的模型清单来源(空/未加载时选择器禁用走默认) */
const aiConfig = useAiConfigStore();

const open = computed({
  get: () => props.open,
  set: (v: boolean) => {
    if (!v) {
      emit("close");
    }
  },
});

/** 已安装的精选 agent(未安装的不进下拉;挂载时探测一次,失败不阻塞) */
const installedAgents = ref<Awaited<ReturnType<typeof agentList>>>([]);
const agentsLoaded = ref(false);
onMounted(() => {
  // 内置模型清单与 agent 清单并行加载,失败均不阻塞对话框
  aiConfig.ensureLoaded().catch(() => {});
  agentList()
    .then((list) => {
      installedAgents.value = list.filter((a) => a.installed);
      agentsLoaded.value = true;
      // 清单晚于配置到达时,补一次归一(配置可能指向已卸载的 agent)
      backend.value = normalizeBackend(backend.value);
    })
    .catch(() => {
      agentsLoaded.value = true;
      backend.value = normalizeBackend(backend.value);
    });
});

// ── 本地副本:打开时从项目 config.json 同步,确认才写回 ────────────────────

const backend = ref("builtin");
const model = ref("");
const thinking = ref("");
/** 页面并发数(1-8;内置后端空 = 设置页全局 AI 并发,agent 后端空 = 默认 2) */
const concurrency = ref<number | "">(2);
const configLoading = ref(false);
const configSaving = ref(false);
const configError = ref("");
let loadSequence = 0;

/** 归一后端选择:仅内置与已安装的精选 agent 合法,其余(含历史遗留的自定义)回退内置 */
function normalizeBackend(value: string): string {
  if (value === "builtin") return value;
  if (value === "custom") return "builtin";
  if (!agentsLoaded.value) return value;
  return installedAgents.value.some((a) => a.id === value) ? value : "builtin";
}

watch(
  () => [props.open, props.projectPath] as const,
  ([isOpen]) => {
    if (!isOpen) {
      return;
    }
    void loadProjectConfig();
  },
);

async function loadProjectConfig() {
  const sequence = ++loadSequence;
  configLoading.value = true;
  configError.value = "";
  backend.value = "builtin";
  model.value = "";
  thinking.value = "";
  concurrency.value = 2;
  try {
    const config = await loadWikiConfig(props.projectPath);
    if (sequence !== loadSequence || !props.open) return;
    if (config.backend.kind === "builtin") {
      backend.value = "builtin";
      model.value = config.backend.model ?? "";
      thinking.value = config.backend.thinking ?? "";
      concurrency.value = config.backend.concurrency
        ? Math.min(8, Math.max(1, config.backend.concurrency))
        : "";
    } else {
      backend.value = normalizeBackend(config.backend.agentId ?? "custom");
      model.value = config.backend.model ?? "";
      thinking.value = config.backend.thinking ?? "";
      concurrency.value = Math.min(8, Math.max(1, config.backend.concurrency ?? 2));
    }
    // 已选中 agent 后端时立即自动获取模型清单(命中缓存则零开销)
    void probeAgent();
  } catch (error) {
    if (sequence === loadSequence) {
      configError.value = error instanceof Error ? error.message : String(error);
    }
  } finally {
    if (sequence === loadSequence) configLoading.value = false;
  }
}

function onBackendChange(value: unknown) {
  if (typeof value === "string" && value !== backend.value) {
    backend.value = value;
    // 切换 agent:模型/思考强度是 per-agent 的选项,清空回到默认,
    // 待新 agent 的清单探测完成后由用户重选(打开时预填走 watch,不经过这里)
    model.value = "";
    thinking.value = "";
  }
}

// ── 模型/思考强度清单自动获取(acpTestCached 会话级缓存) ──────────────────

/** 内置后端可选模型:ai-config 全部厂商/模型,按厂商分组 */
const builtinModelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) return [];
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
});

/** 内置后端的引用是否仍指向现存厂商与模型 */
function builtinModelExists(value: string): boolean {
  const separator = value.indexOf("/");
  if (separator <= 0) return false;
  const provider = aiConfig.config?.providers[value.slice(0, separator)];
  return Boolean(provider?.models.some((m) => m.id === value.slice(separator + 1)));
}

function onBuiltinModelChange(value: unknown) {
  if (typeof value === "string") {
    model.value = value;
  }
}

const probeKey = computed(() => backend.value);

const probeState = ref<{
  key: string;
  loading: boolean;
  failed: boolean;
  error: string;
  result: AcpTestResult | null;
} | null>(null);

async function probeAgent(force = false) {
  const key = probeKey.value;
  if (backend.value === "builtin" || !key) {
    probeState.value = null;
    return;
  }
  // 已在展示同一 key 的结果/进行中状态且非强制时跳过
  if (
    !force &&
    probeState.value?.key === key &&
    (probeState.value.loading || (!probeState.value.failed && probeState.value.result !== null))
  ) {
    return;
  }
  probeState.value = { key, loading: true, failed: false, error: "", result: null };
  try {
    // acpTestCached 命中会话缓存时立即返回,不会重复 spawn
    const result = await acpTestCached(key, { agentId: backend.value }, force);
    if (probeKey.value === key) {
      probeState.value = { key, loading: false, failed: false, error: "", result };
    }
  } catch (error) {
    if (probeKey.value === key) {
      probeState.value = {
        key,
        loading: false,
        failed: true,
        error: error instanceof Error ? error.message : String(error),
        result: null,
      };
    }
  }
}

watch(probeKey, () => {
  void probeAgent();
});

const probeLoading = computed(() => probeState.value?.loading ?? false);
const probeFailed = computed(() => probeState.value?.failed ?? false);

/** 模型下拉选项:优先 config_options 的 model 项,agent 未上报时回退旧式 modes */
const modelChoices = computed(() => {
  const r = probeState.value?.result;
  if (!r) {
    return [];
  }
  const opt = r.configOptions.find((o) => o.category === "model");
  if (opt) {
    return opt.choices;
  }
  return r.modes.map((m) => ({ id: m.id, name: m.name }));
});

/** 思考强度下拉选项:config_options 的 thought_level 项 */
const thinkingChoices = computed(() => {
  const r = probeState.value?.result;
  const opt = r?.configOptions.find((o) => o.category === "thought_level");
  return opt ? opt.choices : [];
});

/** 本地已选值不在上报列表内时补一个原样选项,避免下拉显示空白 */
const modelOptions = computed(() => {
  const list = [...modelChoices.value];
  if (model.value && !list.some((c) => c.id === model.value)) {
    list.push({ id: model.value, name: model.value });
  }
  return list;
});
const thinkingOptions = computed(() => {
  const list = [...thinkingChoices.value];
  if (thinking.value && !list.some((c) => c.id === thinking.value)) {
    list.push({ id: thinking.value, name: thinking.value });
  }
  return list;
});

/** 思考强度下拉:内置后端用 chat 七档;agent 后端用探测到的上报选项 */
const thinkingSelectOptions = computed(() =>
  backend.value === "builtin"
    ? CHAT_THINKING_LEVELS.map((level) => ({
        id: level,
        name: t(`chat.thinkingLevels.${level}`),
      }))
    : thinkingOptions.value,
);

/** 「默认」项文案:内置 = 模型默认;agent = agent 自身配置 */
const thinkingDefaultLabel = computed(() =>
  backend.value === "builtin" ? t("wiki.builtinThinkingDefault") : t("wiki.agentThinkingDefault"),
);

/** 并发数说明:两种后端的默认语义不同 */
const concurrencyHint = computed(() =>
  backend.value === "builtin" ? t("wiki.builtinConcurrencyHint") : t("wiki.agentConcurrencyHint"),
);

const DEFAULT_VALUE = "__default__";

function onModelChange(value: unknown) {
  if (typeof value === "string") {
    model.value = value === DEFAULT_VALUE ? "" : value;
  }
}

function onThinkingChange(value: unknown) {
  if (typeof value === "string") {
    thinking.value = value === DEFAULT_VALUE ? "" : value;
  }
}

/** 探测状态提示行:获取中 / 失败 / agent 未上报任何选项 */
const probeHint = computed(() => {
  if (probeLoading.value) {
    return t("wiki.agentFetchingModels");
  }
  if (probeFailed.value) {
    return t("wiki.agentFetchModelsFailed", { error: probeState.value?.error });
  }
  if (probeState.value?.result && !modelChoices.value.length && !thinkingChoices.value.length) {
    return t("wiki.agentNoModelOptions");
  }
  return "";
});

// ── 提交 ────────────────────────────────────────────────────────────────────

/** edit 模式仅保存配置(右上角入口),generate 模式保存后由调用方触发生成 */
const confirmLabel = computed(() =>
  props.mode === "edit" ? t("common.save") : t("wiki.genConfirm"),
);

async function confirm() {
  const projectPath = props.projectPath;
  const selectedBackend: WikiGenBackend =
    backend.value === "builtin"
      ? {
          kind: "builtin",
          // 失效引用(厂商/模型已从 ai-config 删除)不落盘,回退设置页默认模型
          model: model.value && builtinModelExists(model.value) ? model.value : undefined,
          thinking: thinking.value || undefined,
          concurrency:
            typeof concurrency.value === "number" && concurrency.value > 0
              ? Math.min(8, Math.max(1, Math.round(concurrency.value)))
              : undefined,
        }
      : {
          kind: "agent",
          agentId: backend.value,
          model: model.value || undefined,
          thinking: thinking.value || undefined,
          concurrency: Math.min(8, Math.max(1, Math.round(Number(concurrency.value) || 2))),
        };
  const config: WikiGenerationConfig = { version: 1, backend: selectedBackend };
  configSaving.value = true;
  configError.value = "";
  try {
    await saveWikiConfig(projectPath, config);
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

      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto py-2">
        <!-- 后端:内置 Agent + 已安装的精选 agent(未安装/自定义不展示) -->
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("wiki.genBackend") }}</label>
          <Select
            :model-value="backend"
            :disabled="configLoading || configSaving"
            @update:model-value="onBackendChange"
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="builtin">{{ t("wiki.genBuiltin") }}</SelectItem>
                <SelectItem v-for="a in installedAgents" :key="a.id" :value="a.id">
                  {{ a.name }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>

        <!-- 模型(内置按厂商列 ai-config 全部模型 / agent 用探测清单)/ 思考强度 / 并发数 -->
        <div class="grid gap-3">
          <div class="flex min-w-0 flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("wiki.agentModel") }}</label>
            <ModelSelector
              v-if="backend === 'builtin'"
              :model-value="model"
              :groups="builtinModelGroups"
              :placeholder="t('wiki.builtinModelDefault')"
              :disabled="configLoading || configSaving"
              size="default"
              trigger-class="min-w-0 w-full"
              @update:model-value="onBuiltinModelChange"
            />
            <Select
              v-else
              :model-value="model || DEFAULT_VALUE"
              :disabled="configLoading || configSaving || probeLoading"
              @update:model-value="onModelChange"
            >
              <SelectTrigger class="min-w-0 w-full">
                <SelectValue class="min-w-0 flex-1 truncate text-left" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="DEFAULT_VALUE">
                  {{ t("wiki.agentModelDefault") }}
                </SelectItem>
                <SelectItem v-for="c in modelOptions" :key="c.id" :value="c.id">
                  {{ c.name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div class="flex min-w-0 flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("wiki.agentThinking") }}</label>
            <Select
              :model-value="thinking || DEFAULT_VALUE"
              :disabled="
                configLoading ||
                configSaving ||
                (backend !== 'builtin' && (probeLoading || thinkingChoices.length === 0))
              "
              @update:model-value="onThinkingChange"
            >
              <SelectTrigger class="min-w-0 w-full">
                <SelectValue class="min-w-0 flex-1 truncate text-left" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="DEFAULT_VALUE">{{ thinkingDefaultLabel }}</SelectItem>
                <SelectItem v-for="c in thinkingSelectOptions" :key="c.id" :value="c.id">
                  {{ c.name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div class="flex min-w-0 flex-col gap-1.5">
            <label class="text-sm font-medium">{{ t("wiki.agentConcurrency") }}</label>
            <Input
              v-model.number="concurrency"
              type="number"
              min="1"
              max="8"
              :placeholder="backend === 'builtin' ? t('wiki.builtinConcurrencyPlaceholder') : ''"
              :disabled="configLoading || configSaving"
            />
            <p class="text-xs text-muted-foreground">{{ concurrencyHint }}</p>
          </div>
        </div>
        <p
          v-if="backend !== 'builtin' && probeHint"
          class="flex items-center gap-1.5 text-xs"
          :class="probeFailed ? 'text-amber-600 dark:text-amber-400' : 'text-muted-foreground'"
        >
          <Loader2 v-if="probeLoading" class="h-3 w-3 animate-spin" />
          {{ probeHint }}
        </p>
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
