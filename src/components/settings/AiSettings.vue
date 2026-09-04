<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { FolderOpen, Import, Loader2, Plug, Plus } from "@lucide/vue";
import AiProviderAddDialog from "@/components/settings/AiProviderAddDialog.vue";
import AiProviderCard from "@/components/settings/AiProviderCard.vue";
import CcSwitchImportDialog from "@/components/settings/CcSwitchImportDialog.vue";
import {
  draftModel,
  nextDraftKey,
  type CompatTriState,
  type ModelDraft,
  type ProviderDraft,
} from "@/components/settings/ai-provider-draft";
import {
  ModelSelector,
  modelOptionValue,
  parseModelOptionValue,
  type ModelSelectorGroup,
} from "@/components/ai-elements/model-selector";
import { Button } from "@/components/ui/button";
import { fetchAiModels, testAiConnection } from "@/lib/ai";
import {
  emptyChatPrefs,
  getBuiltinAiProviders,
  revealAiConfigDir,
  type AiModelCompat,
  type AiModelDef,
  type AiModelRef,
  type AiProvider,
  type CcSwitchProvider,
} from "@/lib/ai-config";
import { useAiConfigStore } from "@/stores/ai-config";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n();
const store = useAiConfigStore();
const settings = useSettingsStore();

// ── 本地编辑副本(API Key 类输入不适合边敲边存,点「保存」才全量落盘) ──

const drafts = ref<ProviderDraft[]>([]);
const defaultRef = ref<AiModelRef | null>(null);
const chatPrefs = ref(emptyChatPrefs());
const loading = ref(false);
const saving = ref(false);
const testing = ref(false);
const fetchingKey = ref<string | null>(null);
const confirmDeleteKey = ref<string | null>(null);
const openMap = reactive<Record<string, boolean>>({});

function draftProvider(id: string, provider: AiProvider): ProviderDraft {
  return {
    key: id || nextDraftKey(),
    id,
    name: provider.name,
    api: provider.api,
    baseUrl: provider.baseUrl,
    apiKey: provider.apiKey,
    source: provider,
    models: provider.models.map((model) => draftModel(model)),
  };
}

onMounted(async () => {
  loading.value = true;
  try {
    // force:设置页打开即拉取最新配置,覆盖外部对配置文件的修改
    const config = await store.ensureLoaded(true);
    defaultRef.value = config.defaultModel;
    chatPrefs.value = { ...emptyChatPrefs(), ...config.chat };
    drafts.value = Object.entries(config.providers).map(([id, provider]) =>
      draftProvider(id, provider),
    );
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    loading.value = false;
  }
  // 内置厂商目录仅供添加对话框使用,拉取失败不阻断设置页
  try {
    builtinCatalog.value = await getBuiltinAiProviders();
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  }
});

// ── 添加厂商 / 从 CC Switch 导入(对话框抽为子组件,确认后并入草稿) ────

const addDialogOpen = ref(false);
const builtinCatalog = ref<Record<string, AiProvider>>({});
const ccSwitchDialogOpen = ref(false);

/** 已占用的厂商 id(传给添加对话框做去重与选项过滤) */
const existingProviderIds = computed(() => drafts.value.map((draft) => draft.id.trim()));

function onAddProvider(payload: {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  api: AiProvider["api"];
  models: AiModelDef[];
}) {
  const draft: ProviderDraft = {
    key: nextDraftKey(),
    id: payload.id,
    name: payload.name,
    api: payload.api,
    baseUrl: payload.baseUrl,
    apiKey: payload.apiKey,
    models: payload.models.map((model) => draftModel(model)),
  };
  drafts.value = [...drafts.value, draft];
  openMap[draft.key] = true;
}

/** 为导入的厂商生成不与现有草稿冲突的 id(重复时追加 -2/-3 后缀) */
function uniqueProviderId(base: string): string {
  const used = new Set(drafts.value.map((draft) => draft.id.trim()));
  const root = base.trim() || "imported";
  if (!used.has(root)) return root;
  for (let index = 2; ; index++) {
    const candidate = `${root}-${index}`;
    if (!used.has(candidate)) return candidate;
  }
}

function onCcSwitchImport(providers: CcSwitchProvider[]) {
  for (const provider of providers) {
    const draft: ProviderDraft = {
      key: nextDraftKey(),
      id: uniqueProviderId(provider.id),
      name: provider.name.trim(),
      api: provider.api,
      baseUrl: provider.baseUrl.trim(),
      apiKey: provider.apiKey.trim(),
      models: provider.models.map((model) => draftModel(model)),
    };
    drafts.value = [...drafts.value, draft];
    openMap[draft.key] = true;
  }
  toast.success(t("settings.ai.ccSwitchImported", { count: providers.length }));
}

// ── 厂商删除 ─────────────────────────────────────────────────────────

function removeProvider(draft: ProviderDraft) {
  // 两段式删除:第一次点击进入确认态,3 秒内再次点击才真正删除
  if (confirmDeleteKey.value !== draft.key) {
    confirmDeleteKey.value = draft.key;
    setTimeout(() => {
      if (confirmDeleteKey.value === draft.key) confirmDeleteKey.value = null;
    }, 3000);
    return;
  }
  confirmDeleteKey.value = null;
  drafts.value = drafts.value.filter((item) => item !== draft);
  delete openMap[draft.key];
}

// ── 模型 ID 下拉候选(「获取模型列表」的结果,供填写时挑选,不直接落行) ──

/** 各厂商最近一次拉取到的模型 ID 列表(按厂商本地 key 存放) */
const fetchedModels = reactive<Record<string, string[]>>({});

/** 拉取厂商模型列表,作为模型 ID 输入框的下拉候选 */
async function fetchModels(draft: ProviderDraft) {
  if (fetchingKey.value) return;
  fetchingKey.value = draft.key;
  try {
    const list = await fetchAiModels(draft.baseUrl.trim(), draft.apiKey.trim(), draft.api);
    fetchedModels[draft.key] = list.filter((id) => id.trim());
    toast.success(t("settings.ai.fetchModelsSuccess", { count: list.length }));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    toast.error(t("settings.ai.fetchModelsFailed", { error: message }));
  } finally {
    fetchingKey.value = null;
  }
}

// ── 默认模型 ─────────────────────────────────────────────────────────

const modelGroups = computed<ModelSelectorGroup[]>(() =>
  drafts.value
    .filter((draft) => draft.id.trim())
    .map((draft) => ({
      providerId: draft.id.trim(),
      providerName: draft.name.trim() || draft.id.trim(),
      models: draft.models
        .filter((model) => model.id.trim())
        .map((model) => ({
          ...model.source,
          id: model.id.trim(),
          api: model.api || undefined,
          name: model.name.trim(),
          reasoning: true,
        })),
    })),
);

const defaultModelValue = computed(() =>
  defaultRef.value ? modelOptionValue(defaultRef.value.providerId, defaultRef.value.modelId) : "",
);

function onDefaultModelChange(value: string) {
  defaultRef.value = parseModelOptionValue(value);
}

function isDefaultProvider(draft: ProviderDraft): boolean {
  const reference = defaultRef.value;
  return Boolean(
    reference &&
    reference.providerId === draft.id.trim() &&
    draft.models.some((model) => model.id.trim() === reference.modelId),
  );
}

// ── 保存 / 测试 / 打开配置目录 ───────────────────────────────────────

function parseTokenCount(value: string): number {
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
}

/**
 * 由三态草稿合成 compat:auto 不落字段(回退自动探测);
 * 保留 UI 未暴露的既有字段(thinkingFormat 等),全 auto 且无残留时返回 undefined。
 */
function buildCompat(model: ModelDraft): AiModelCompat | undefined {
  const compat: AiModelCompat = { ...model.source.compat };
  const applyTri = (
    key: "supportsDeveloperRole" | "supportsReasoningEffort" | "supportsStore",
    state: CompatTriState,
  ) => {
    if (state === "auto") delete compat[key];
    else compat[key] = state === "on";
  };
  applyTri("supportsDeveloperRole", model.compat.supportsDeveloperRole);
  applyTri("supportsReasoningEffort", model.compat.supportsReasoningEffort);
  applyTri("supportsStore", model.compat.supportsStore);
  if (model.compat.maxTokensField === "auto") delete compat.maxTokensField;
  else compat.maxTokensField = model.compat.maxTokensField;
  return Object.keys(compat).length > 0 ? compat : undefined;
}

function buildProviders(): Record<string, AiProvider> | null {
  const providers: Record<string, AiProvider> = {};
  for (const draft of drafts.value) {
    const id = draft.id.trim();
    if (!id) {
      toast.error(t("settings.ai.missingProviderId"));
      return null;
    }
    if (providers[id]) {
      toast.error(t("settings.ai.duplicateProviderId", { id }));
      return null;
    }
    providers[id] = {
      name: draft.name.trim(),
      baseUrl: draft.baseUrl.trim().replace(/\/+$/, ""),
      apiKey: draft.apiKey.trim(),
      api: draft.api,
      headers: draft.source?.headers,
      models: draft.models
        .filter((model) => model.id.trim())
        .map((model) => ({
          ...model.source,
          id: model.id.trim(),
          api: model.api || undefined,
          name: model.name.trim(),
          reasoning: true,
          contextWindow: parseTokenCount(model.contextWindow),
          maxTokens: parseTokenCount(model.maxTokens),
          // undefined 时 JSON 序列化丢弃该键,配置回到全自动探测
          compat: buildCompat(model),
        })),
    };
  }
  return providers;
}

async function save(): Promise<void> {
  const providers = buildProviders();
  if (!providers) return;
  const base = store.config;
  if (!base) return;
  saving.value = true;
  try {
    await store.save({
      version: base.version,
      providers,
      defaultModel: defaultRef.value,
      chat: { ...chatPrefs.value },
    });
    // 按后端归一化结果刷新本地副本(悬空引用会被清掉,模型行重排)
    const fresh = await store.reload();
    defaultRef.value = fresh.defaultModel;
    chatPrefs.value = { ...emptyChatPrefs(), ...fresh.chat };
    drafts.value = Object.entries(fresh.providers).map(([id, provider]) =>
      draftProvider(id, provider),
    );
    toast.success(t("settings.ai.saved"));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    toast.error(message);
  } finally {
    saving.value = false;
  }
}

async function testConnection() {
  if (testing.value) return;
  testing.value = true;
  try {
    // 先落库当前表单值,测试使用的就是界面上看到的默认模型
    await save();
    await testAiConnection();
    toast.success(t("settings.ai.testSuccess"));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    toast.error(t("settings.ai.testFailed", { error: message }));
  } finally {
    testing.value = false;
  }
}

async function revealConfig() {
  try {
    await revealAiConfigDir();
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  }
}

// AI 调用并发上限(1-5),点选即持久化
const CONCURRENCY_OPTIONS = [1, 2, 3, 4, 5];
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.ai.title") }}</h2>
    <p class="text-muted-foreground mt-1 text-sm">{{ t("settings.ai.description") }}</p>

    <div class="mt-4 flex flex-col gap-4">
      <!-- 默认模型 -->
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium">{{ t("settings.ai.defaultModel") }}</label>
        <ModelSelector
          :model-value="defaultModelValue"
          :groups="modelGroups"
          size="default"
          trigger-class="w-full justify-between"
          :placeholder="t('settings.ai.defaultModelPlaceholder')"
          :disabled="loading"
          @update:model-value="onDefaultModelChange"
        />
        <p class="text-muted-foreground text-xs">{{ t("settings.ai.defaultModelHint") }}</p>
      </div>

      <!-- 厂商列表 -->
      <div class="flex flex-col gap-1.5">
        <div class="flex items-center justify-between">
          <label class="text-sm font-medium">{{ t("settings.ai.providers") }}</label>
          <div class="flex items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              class="h-7 gap-1"
              @click="ccSwitchDialogOpen = true"
            >
              <Import class="h-3.5 w-3.5" />
              {{ t("settings.ai.importCcSwitch") }}
            </Button>
            <Button variant="outline" size="sm" class="h-7 gap-1" @click="addDialogOpen = true">
              <Plus class="h-3.5 w-3.5" />
              {{ t("settings.ai.addProvider") }}
            </Button>
          </div>
        </div>

        <p v-if="!drafts.length" class="text-muted-foreground text-xs">
          {{ t("settings.ai.emptyProviders") }}
        </p>

        <div class="flex flex-col gap-2">
          <AiProviderCard
            v-for="draft in drafts"
            :key="draft.key"
            :draft="draft"
            :open="openMap[draft.key] ?? false"
            :confirm-delete="confirmDeleteKey === draft.key"
            :is-default="isDefaultProvider(draft)"
            :fetching-key="fetchingKey"
            :fetched-model-ids="fetchedModels[draft.key] ?? []"
            @update:open="openMap[draft.key] = $event"
            @remove="removeProvider(draft)"
            @fetch-models="fetchModels(draft)"
          />
        </div>
      </div>

      <div class="flex items-center gap-2">
        <Button size="sm" :disabled="saving || loading" @click="save">
          <Loader2 v-if="saving" class="h-3.5 w-3.5 animate-spin" />
          {{ t("common.save") }}
        </Button>
        <Button
          size="sm"
          variant="outline"
          class="gap-1.5"
          :disabled="testing || saving || loading"
          @click="testConnection"
        >
          <Loader2 v-if="testing" class="h-3.5 w-3.5 animate-spin" />
          <Plug v-else class="h-3.5 w-3.5" />
          {{ testing ? t("settings.ai.testing") : t("settings.ai.test") }}
        </Button>
      </div>

      <!-- 并发上限 -->
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium">{{ t("settings.ai.concurrency") }}</label>
        <div class="flex gap-1.5">
          <button
            v-for="n in CONCURRENCY_OPTIONS"
            :key="n"
            type="button"
            class="h-8 w-8 rounded-md border text-sm transition-colors"
            :class="
              settings.aiConcurrency === n
                ? 'border-primary bg-primary/10 font-medium'
                : 'hover:bg-accent'
            "
            @click="settings.setAiConcurrency(n)"
          >
            {{ n }}
          </button>
        </div>
        <p class="text-muted-foreground text-xs">
          {{ t("settings.ai.concurrencyHint") }}
        </p>
      </div>

      <!-- 配置文件 -->
      <div class="text-muted-foreground flex items-center justify-between gap-2 text-xs">
        <span class="truncate">
          {{ t("settings.ai.configFile") }}
          <code class="bg-muted rounded px-1 py-0.5">~/.repomeow/ai-config.json</code>
        </span>
        <Button variant="ghost" size="sm" class="h-7 shrink-0 gap-1 text-xs" @click="revealConfig">
          <FolderOpen class="h-3.5 w-3.5" />
          {{ t("settings.ai.openConfigDir") }}
        </Button>
      </div>
    </div>

    <!-- 添加厂商对话框:先选内置厂商(带入地址与预置模型)或自定义,再补 API Key -->
    <AiProviderAddDialog
      v-model:open="addDialogOpen"
      :catalog="builtinCatalog"
      :existing-ids="existingProviderIds"
      @add="onAddProvider"
    />

    <!-- 从 CC Switch 导入对话框:列出本机 ~/.cc-switch 中 OpenAI chat 兼容的供应商,勾选后并入草稿 -->
    <CcSwitchImportDialog v-model:open="ccSwitchDialogOpen" @import="onCcSwitchImport" />
  </section>
</template>
