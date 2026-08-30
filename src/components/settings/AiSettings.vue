<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { ChevronDown, FolderOpen, Loader2, Plug, Plus, RefreshCw, Trash2 } from "@lucide/vue";
import {
  ModelSelector,
  modelOptionValue,
  parseModelOptionValue,
  type ModelSelectorGroup,
} from "@/components/ai-elements/model-selector";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { fetchAiModels, testAiConnection } from "@/lib/ai";
import {
  emptyChatPrefs,
  revealAiConfigDir,
  type AiModelDef,
  type AiModelRef,
  type AiProvider,
} from "@/lib/ai-config";
import { useAiConfigStore } from "@/stores/ai-config";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n();
const store = useAiConfigStore();
const settings = useSettingsStore();

// ── 本地编辑副本(API Key 类输入不适合边敲边存,点「保存」才全量落盘) ──

interface ModelDraft {
  key: string;
  id: string;
  name: string;
  contextWindow: string;
  maxTokens: string;
  reasoning: boolean;
  /** 建控件之外的原始定义(cost/compat/input 等透传保存,避免 UI 字段丢元数据) */
  source: AiModelDef;
}

interface ProviderDraft {
  key: string;
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  models: ModelDraft[];
}

const drafts = ref<ProviderDraft[]>([]);
const defaultRef = ref<AiModelRef | null>(null);
const chatPrefs = ref(emptyChatPrefs());
const loading = ref(false);
const saving = ref(false);
const testing = ref(false);
const fetchingKey = ref<string | null>(null);
const confirmDeleteKey = ref<string | null>(null);
const openMap = reactive<Record<string, boolean>>({});

let localKey = 0;
const nextKey = () => `local-${localKey++}`;

function draftProvider(id: string, provider: AiProvider): ProviderDraft {
  return {
    key: id || nextKey(),
    id,
    name: provider.name,
    baseUrl: provider.baseUrl,
    apiKey: provider.apiKey,
    models: provider.models.map((model) => draftModel(model)),
  };
}

function draftModel(model: AiModelDef): ModelDraft {
  return {
    key: nextKey(),
    id: model.id,
    name: model.name,
    contextWindow: model.contextWindow > 0 ? String(model.contextWindow) : "",
    maxTokens: model.maxTokens > 0 ? String(model.maxTokens) : "",
    reasoning: model.reasoning,
    source: model,
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
});

// ── 厂商增删 ─────────────────────────────────────────────────────────

function addProvider() {
  const draft: ProviderDraft = {
    key: nextKey(),
    id: "",
    name: "",
    baseUrl: "",
    apiKey: "",
    models: [],
  };
  drafts.value = [...drafts.value, draft];
  openMap[draft.key] = true;
}

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

// ── 模型行 ───────────────────────────────────────────────────────────

function addModel(draft: ProviderDraft) {
  draft.models = [
    ...draft.models,
    draftModel({
      id: "",
      name: "",
      reasoning: false,
      input: ["text"],
      contextWindow: 0,
      maxTokens: 0,
    }),
  ];
}

function removeModel(draft: ProviderDraft, model: ModelDraft) {
  draft.models = draft.models.filter((item) => item !== model);
}

/** 拉取厂商模型列表并与本地行合并(已存在的 id 保留本地编辑) */
async function fetchModels(draft: ProviderDraft) {
  if (fetchingKey.value) return;
  fetchingKey.value = draft.key;
  try {
    const list = await fetchAiModels(draft.baseUrl.trim(), draft.apiKey.trim());
    const existing = new Set(draft.models.map((model) => model.id.trim()));
    for (const id of list) {
      if (!id || existing.has(id)) continue;
      existing.add(id);
      draft.models = [
        ...draft.models,
        draftModel({
          id,
          name: "",
          reasoning: false,
          input: ["text"],
          contextWindow: 0,
          maxTokens: 0,
        }),
      ];
    }
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
          name: model.name.trim(),
          reasoning: model.reasoning,
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
      api: "openai-completions",
      models: draft.models
        .filter((model) => model.id.trim())
        .map((model) => ({
          ...model.source,
          id: model.id.trim(),
          name: model.name.trim(),
          reasoning: model.reasoning,
          contextWindow: parseTokenCount(model.contextWindow),
          maxTokens: parseTokenCount(model.maxTokens),
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
          <Button variant="outline" size="sm" class="h-7 gap-1" @click="addProvider">
            <Plus class="h-3.5 w-3.5" />
            {{ t("settings.ai.addProvider") }}
          </Button>
        </div>

        <p v-if="!drafts.length" class="text-muted-foreground text-xs">
          {{ t("settings.ai.emptyProviders") }}
        </p>

        <div class="flex flex-col gap-2">
          <Collapsible
            v-for="draft in drafts"
            :key="draft.key"
            :open="openMap[draft.key]"
            @update:open="openMap[draft.key] = $event"
          >
            <div class="rounded-lg border">
              <div class="flex items-center gap-2 px-2 py-1.5">
                <CollapsibleTrigger as-child>
                  <button
                    type="button"
                    class="hover:bg-accent flex min-w-0 flex-1 items-center gap-2 rounded-md px-1 py-1 text-left"
                  >
                    <ChevronDown
                      class="text-muted-foreground h-3.5 w-3.5 shrink-0 transition-transform"
                      :class="openMap[draft.key] ? 'rotate-180' : ''"
                    />
                    <span class="truncate text-sm font-medium">
                      {{ draft.name.trim() || draft.id.trim() || t("settings.ai.unnamedProvider") }}
                    </span>
                    <span class="text-muted-foreground truncate text-xs">
                      {{ draft.baseUrl.trim() }}
                    </span>
                  </button>
                </CollapsibleTrigger>
                <span
                  v-if="isDefaultProvider(draft)"
                  class="bg-primary/10 text-primary shrink-0 rounded-full px-2 py-0.5 text-xs font-medium"
                >
                  {{ t("settings.ai.defaultBadge") }}
                </span>
                <span class="text-muted-foreground shrink-0 text-xs">
                  {{ t("settings.ai.modelCount", { count: draft.models.length }) }}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-7 shrink-0 px-2 text-xs"
                  :class="
                    confirmDeleteKey === draft.key ? 'text-destructive' : 'text-muted-foreground'
                  "
                  :title="t('settings.ai.deleteProvider')"
                  @click="removeProvider(draft)"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                  {{
                    confirmDeleteKey === draft.key
                      ? t("settings.ai.confirmDeleteProvider")
                      : t("settings.ai.deleteProvider")
                  }}
                </Button>
              </div>

              <CollapsibleContent>
                <div class="flex flex-col gap-3 border-t px-3 py-3">
                  <div class="grid grid-cols-2 gap-2">
                    <div class="flex flex-col gap-1">
                      <label class="text-muted-foreground text-xs">{{
                        t("settings.ai.providerId")
                      }}</label>
                      <Input
                        v-model="draft.id"
                        class="h-8 text-xs"
                        :placeholder="t('settings.ai.providerIdPlaceholder')"
                        spellcheck="false"
                      />
                    </div>
                    <div class="flex flex-col gap-1">
                      <label class="text-muted-foreground text-xs">{{
                        t("settings.ai.providerName")
                      }}</label>
                      <Input
                        v-model="draft.name"
                        class="h-8 text-xs"
                        :placeholder="t('settings.ai.providerNamePlaceholder')"
                        spellcheck="false"
                      />
                    </div>
                  </div>
                  <div class="flex flex-col gap-1">
                    <label class="text-muted-foreground text-xs">{{
                      t("settings.ai.baseUrl")
                    }}</label>
                    <Input
                      v-model="draft.baseUrl"
                      class="h-8 text-xs"
                      :placeholder="t('settings.ai.baseUrlPlaceholder')"
                      spellcheck="false"
                    />
                  </div>
                  <div class="flex flex-col gap-1">
                    <label class="text-muted-foreground text-xs">{{
                      t("settings.ai.apiKey")
                    }}</label>
                    <Input
                      v-model="draft.apiKey"
                      type="password"
                      class="h-8 text-xs"
                      :placeholder="t('settings.ai.apiKeyPlaceholder')"
                      autocomplete="off"
                      spellcheck="false"
                    />
                  </div>

                  <!-- 模型行 -->
                  <div class="flex flex-col gap-1.5">
                    <div class="flex items-center justify-between">
                      <label class="text-muted-foreground text-xs">{{
                        t("settings.ai.models")
                      }}</label>
                      <Button
                        variant="outline"
                        size="sm"
                        class="h-7 gap-1"
                        :disabled="
                          fetchingKey != null || !draft.baseUrl.trim() || !draft.apiKey.trim()
                        "
                        :title="t('settings.ai.fetchModelsHint')"
                        @click="fetchModels(draft)"
                      >
                        <Loader2 v-if="fetchingKey === draft.key" class="h-3 w-3 animate-spin" />
                        <RefreshCw v-else class="h-3 w-3" />
                        {{
                          fetchingKey === draft.key
                            ? t("settings.ai.fetchingModels")
                            : t("settings.ai.fetchModels")
                        }}
                      </Button>
                    </div>
                    <p v-if="!draft.models.length" class="text-muted-foreground text-xs">
                      {{ t("settings.ai.noModels") }}
                    </p>
                    <div
                      v-for="model in draft.models"
                      :key="model.key"
                      class="flex flex-wrap items-center gap-1.5 rounded-md border p-1.5"
                    >
                      <Input
                        v-model="model.id"
                        class="h-7 min-w-36 flex-1 text-xs"
                        :placeholder="t('settings.ai.modelIdPlaceholder')"
                        spellcheck="false"
                      />
                      <Input
                        v-model="model.name"
                        class="h-7 w-28 text-xs"
                        :placeholder="t('settings.ai.modelNamePlaceholder')"
                        spellcheck="false"
                      />
                      <Input
                        v-model="model.contextWindow"
                        class="h-7 w-24 text-xs tabular-nums"
                        :placeholder="t('settings.ai.contextWindow')"
                        :title="t('settings.ai.contextWindow')"
                      />
                      <Input
                        v-model="model.maxTokens"
                        class="h-7 w-20 text-xs tabular-nums"
                        :placeholder="t('settings.ai.maxTokens')"
                        :title="t('settings.ai.maxTokens')"
                      />
                      <label class="text-muted-foreground flex items-center gap-1 text-xs">
                        <Switch v-model="model.reasoning" />
                        <span>{{ t("settings.ai.reasoning") }}</span>
                      </label>
                      <Button
                        variant="ghost"
                        size="icon"
                        class="text-muted-foreground hover:text-destructive h-7 w-7"
                        :title="t('settings.ai.removeModel')"
                        @click="removeModel(draft, model)"
                      >
                        <Trash2 class="h-3.5 w-3.5" />
                      </Button>
                    </div>
                    <Button
                      variant="outline"
                      size="sm"
                      class="h-7 w-fit gap-1 text-xs"
                      @click="addModel(draft)"
                    >
                      <Plus class="h-3 w-3" />
                      {{ t("settings.ai.addModel") }}
                    </Button>
                  </div>
                </div>
              </CollapsibleContent>
            </div>
          </Collapsible>
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
  </section>
</template>
