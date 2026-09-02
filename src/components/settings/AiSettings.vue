<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref, type ComponentPublicInstance } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  Check,
  ChevronDown,
  FolderOpen,
  Import,
  Loader2,
  Plug,
  Plus,
  RefreshCw,
  Trash2,
} from "@lucide/vue";
import {
  ModelSelector,
  modelOptionValue,
  parseModelOptionValue,
  type ModelSelectorGroup,
} from "@/components/ai-elements/model-selector";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { fetchAiModels, testAiConnection } from "@/lib/ai";
import {
  emptyChatPrefs,
  getBuiltinAiProviders,
  listCcSwitchProviders,
  revealAiConfigDir,
  type AiModelDef,
  type AiModelRef,
  type AiProvider,
  type CcSwitchProvider,
  type CcSwitchScan,
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
  // 内置厂商目录仅供添加对话框使用,拉取失败不阻断设置页
  try {
    builtinCatalog.value = await getBuiltinAiProviders();
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  }
});

// ── 添加厂商对话框 ───────────────────────────────────────────────────

/** 厂商选择中「自定义」选项的固定值 */
const CUSTOM_CHOICE = "custom";

const addDialogOpen = ref(false);
const builtinCatalog = ref<Record<string, AiProvider>>({});
const addChoice = ref(CUSTOM_CHOICE);
const addForm = reactive({ id: "", name: "", baseUrl: "", apiKey: "" });

/** 内置目录中尚未添加的厂商(已存在的 id 不再提供,避免重复) */
const addOptions = computed(() => {
  const used = new Set(drafts.value.map((draft) => draft.id.trim()));
  return Object.entries(builtinCatalog.value)
    .filter(([id]) => !used.has(id))
    .map(([id, provider]) => ({ id, name: provider.name.trim() || id }));
});

/** 选中内置厂商时带入的预置模型数(对话框提示用) */
const seededModelCount = computed(() =>
  addChoice.value === CUSTOM_CHOICE
    ? 0
    : (builtinCatalog.value[addChoice.value]?.models.length ?? 0),
);

function openAddDialog() {
  addChoice.value = CUSTOM_CHOICE;
  addForm.id = "";
  addForm.name = "";
  addForm.baseUrl = "";
  addForm.apiKey = "";
  addDialogOpen.value = true;
}

function onAddChoiceChange(value: unknown) {
  const choice = String(value);
  addChoice.value = choice;
  if (choice === CUSTOM_CHOICE) {
    addForm.id = "";
    addForm.name = "";
    addForm.baseUrl = "";
    return;
  }
  const provider = builtinCatalog.value[choice];
  if (!provider) return;
  addForm.id = choice;
  addForm.name = provider.name;
  addForm.baseUrl = provider.baseUrl;
}

function confirmAddProvider() {
  const id = addForm.id.trim();
  if (!id) {
    toast.error(t("settings.ai.missingProviderId"));
    return;
  }
  if (drafts.value.some((draft) => draft.id.trim() === id)) {
    toast.error(t("settings.ai.duplicateProviderId", { id }));
    return;
  }
  const catalogModels =
    addChoice.value === CUSTOM_CHOICE ? [] : (builtinCatalog.value[addChoice.value]?.models ?? []);
  const draft: ProviderDraft = {
    key: nextKey(),
    id,
    name: addForm.name.trim(),
    baseUrl: addForm.baseUrl.trim(),
    apiKey: addForm.apiKey.trim(),
    models: catalogModels.map((model) => draftModel(model)),
  };
  drafts.value = [...drafts.value, draft];
  openMap[draft.key] = true;
  addDialogOpen.value = false;
}

// ── 从 CC Switch 导入 ───────────────────────────────────────────────

const ccSwitchDialogOpen = ref(false);
const ccSwitchLoading = ref(false);
const ccSwitchScan = ref<CcSwitchScan | null>(null);
/** 勾选项的 key:`${app}/${id}`(不同应用间 id 可能重复) */
const ccSwitchChecked = ref<Set<string>>(new Set());

function ccSwitchKey(provider: CcSwitchProvider): string {
  return `${provider.app}/${provider.id}`;
}

async function openCcSwitchDialog() {
  ccSwitchDialogOpen.value = true;
  ccSwitchLoading.value = true;
  ccSwitchScan.value = null;
  try {
    const scan = await listCcSwitchProviders();
    ccSwitchScan.value = scan;
    // 默认勾选「当前启用」的供应商,其余由用户自行挑选
    ccSwitchChecked.value = new Set(
      scan.providers.filter((provider) => provider.current).map(ccSwitchKey),
    );
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
    ccSwitchDialogOpen.value = false;
  } finally {
    ccSwitchLoading.value = false;
  }
}

function toggleCcSwitchProvider(provider: CcSwitchProvider) {
  const key = ccSwitchKey(provider);
  const next = new Set(ccSwitchChecked.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  ccSwitchChecked.value = next;
}

const ccSwitchAllChecked = computed(() => {
  const providers = ccSwitchScan.value?.providers ?? [];
  return (
    providers.length > 0 &&
    providers.every((provider) => ccSwitchChecked.value.has(ccSwitchKey(provider)))
  );
});

function toggleCcSwitchAll() {
  const providers = ccSwitchScan.value?.providers ?? [];
  ccSwitchChecked.value = ccSwitchAllChecked.value
    ? new Set()
    : new Set(providers.map(ccSwitchKey));
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

function confirmCcSwitchImport() {
  const providers = (ccSwitchScan.value?.providers ?? []).filter((provider) =>
    ccSwitchChecked.value.has(ccSwitchKey(provider)),
  );
  if (!providers.length) return;
  for (const provider of providers) {
    const draft: ProviderDraft = {
      key: nextKey(),
      id: uniqueProviderId(provider.id),
      name: provider.name.trim(),
      baseUrl: provider.baseUrl.trim(),
      apiKey: provider.apiKey.trim(),
      models: provider.models.map((model) => draftModel(model)),
    };
    drafts.value = [...drafts.value, draft];
    openMap[draft.key] = true;
  }
  ccSwitchDialogOpen.value = false;
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

// ── 模型行 ───────────────────────────────────────────────────────────

function addModel(draft: ProviderDraft) {
  draft.models = [
    ...draft.models,
    draftModel({
      id: "",
      name: "",
      reasoning: true,
      input: ["text"],
      contextWindow: 0,
      maxTokens: 0,
    }),
  ];
}

function removeModel(draft: ProviderDraft, model: ModelDraft) {
  draft.models = draft.models.filter((item) => item !== model);
}

// ── 模型 ID 下拉候选(「获取模型列表」的结果,供填写时挑选,不直接落行) ──

/** 各厂商最近一次拉取到的模型 ID 列表(按厂商本地 key 存放) */
const fetchedModels = reactive<Record<string, string[]>>({});
/** 各模型行 ID 输入框的候选弹层开关(仅由行内下拉按钮开合) */
const suggestionOpen = reactive<Record<string, boolean>>({});
/** 弹层顶部搜索框文本(弹层关闭时删除,打开时重置) */
const suggestionQuery = reactive<Record<string, string>>({});
const suggestionSearchRefs = new Map<string, HTMLInputElement>();
const SUGGESTION_LIMIT = 50;

/** 拉取厂商模型列表,作为模型 ID 输入框的下拉候选 */
async function fetchModels(draft: ProviderDraft) {
  if (fetchingKey.value) return;
  fetchingKey.value = draft.key;
  try {
    const list = await fetchAiModels(draft.baseUrl.trim(), draft.apiKey.trim());
    fetchedModels[draft.key] = list.filter((id) => id.trim());
    toast.success(t("settings.ai.fetchModelsSuccess", { count: list.length }));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    toast.error(t("settings.ai.fetchModelsFailed", { error: message }));
  } finally {
    fetchingKey.value = null;
  }
}

/** 该厂商是否已有拉取到的模型候选(决定模型行是否展示下拉按钮) */
function hasFetchedModels(draft: ProviderDraft): boolean {
  return (fetchedModels[draft.key]?.length ?? 0) > 0;
}

/** 当前行的下拉候选:按弹层搜索词模糊过滤、排除其他行已占用的 ID */
function modelSuggestions(draft: ProviderDraft, model: ModelDraft): string[] {
  const list = fetchedModels[draft.key];
  if (!list?.length) return [];
  const used = new Set(draft.models.filter((item) => item !== model).map((item) => item.id.trim()));
  const keyword = (suggestionQuery[model.key] ?? "").trim().toLowerCase();
  return list
    .filter((id) => !used.has(id) && (!keyword || id.toLowerCase().includes(keyword)))
    .slice(0, SUGGESTION_LIMIT);
}

function setSuggestionSearchRef(key: string, el: unknown) {
  const input = (el as ComponentPublicInstance | null)?.$el;
  if (input instanceof HTMLInputElement) suggestionSearchRefs.set(key, input);
  else suggestionSearchRefs.delete(key);
}

/** 弹层开合统一入口:展开时重置搜索词并聚焦搜索框,关闭时清理 */
function onSuggestionOpenChange(model: ModelDraft, open: boolean) {
  suggestionOpen[model.key] = open;
  if (!open) {
    delete suggestionQuery[model.key];
    return;
  }
  suggestionQuery[model.key] = "";
  nextTick(() => suggestionSearchRefs.get(model.key)?.focus());
}

function pickModelId(model: ModelDraft, id: string) {
  model.id = id;
  suggestionOpen[model.key] = false;
  delete suggestionQuery[model.key];
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
          reasoning: true,
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
          <div class="flex items-center gap-1.5">
            <Button variant="outline" size="sm" class="h-7 gap-1" @click="openCcSwitchDialog">
              <Import class="h-3.5 w-3.5" />
              {{ t("settings.ai.importCcSwitch") }}
            </Button>
            <Button variant="outline" size="sm" class="h-7 gap-1" @click="openAddDialog">
              <Plus class="h-3.5 w-3.5" />
              {{ t("settings.ai.addProvider") }}
            </Button>
          </div>
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
                      class="flex flex-col gap-2 rounded-md border p-2"
                    >
                      <Popover
                        :open="suggestionOpen[model.key] ?? false"
                        @update:open="onSuggestionOpenChange(model, $event)"
                      >
                        <div class="flex items-end gap-1.5">
                          <div class="flex min-w-0 flex-1 flex-col gap-1">
                            <label class="text-muted-foreground text-xs">{{
                              t("settings.ai.modelId")
                            }}</label>
                            <PopoverAnchor as-child>
                              <Input
                                v-model="model.id"
                                class="h-7 text-xs"
                                :placeholder="t('settings.ai.modelIdPlaceholder')"
                                spellcheck="false"
                              />
                            </PopoverAnchor>
                          </div>
                          <PopoverTrigger
                            v-if="hasFetchedModels(draft)"
                            as-child
                            :title="t('settings.ai.pickFetchedModel')"
                          >
                            <Button variant="outline" size="icon" class="h-7 w-7 shrink-0">
                              <ChevronDown
                                class="h-3.5 w-3.5 transition-transform"
                                :class="suggestionOpen[model.key] ? 'rotate-180' : ''"
                              />
                            </Button>
                          </PopoverTrigger>
                          <Button
                            variant="ghost"
                            size="icon"
                            class="text-muted-foreground hover:text-destructive h-7 w-7 shrink-0"
                            :title="t('settings.ai.removeModel')"
                            @click="removeModel(draft, model)"
                          >
                            <Trash2 class="h-3.5 w-3.5" />
                          </Button>
                        </div>
                        <PopoverContent
                          class="w-(--reka-popper-anchor-width) gap-1 p-1"
                          align="start"
                          @open-auto-focus.prevent
                          @close-auto-focus.prevent
                        >
                          <Input
                            :ref="(el) => setSuggestionSearchRef(model.key, el)"
                            :model-value="suggestionQuery[model.key] ?? ''"
                            class="h-7 text-xs"
                            :placeholder="t('settings.ai.searchFetchedModels')"
                            spellcheck="false"
                            @update:model-value="suggestionQuery[model.key] = String($event)"
                          />
                          <div class="max-h-56 overflow-y-auto">
                            <button
                              v-for="option in modelSuggestions(draft, model)"
                              :key="option"
                              type="button"
                              class="hover:bg-accent w-full truncate rounded-sm px-2 py-1 text-left text-xs"
                              @click="pickModelId(model, option)"
                            >
                              {{ option }}
                            </button>
                            <p
                              v-if="!modelSuggestions(draft, model).length"
                              class="text-muted-foreground px-2 py-1.5 text-xs"
                            >
                              {{ t("settings.ai.noMatchedModels") }}
                            </p>
                          </div>
                        </PopoverContent>
                      </Popover>
                      <div class="grid grid-cols-3 gap-1.5">
                        <div class="flex flex-col gap-1">
                          <label class="text-muted-foreground text-xs">{{
                            t("settings.ai.modelName")
                          }}</label>
                          <Input
                            v-model="model.name"
                            class="h-7 text-xs"
                            :placeholder="t('settings.ai.modelNamePlaceholder')"
                            spellcheck="false"
                          />
                        </div>
                        <div class="flex flex-col gap-1">
                          <label class="text-muted-foreground text-xs">{{
                            t("settings.ai.contextWindow")
                          }}</label>
                          <Input
                            v-model="model.contextWindow"
                            class="h-7 text-xs tabular-nums"
                            :placeholder="t('settings.ai.contextWindowPlaceholder')"
                          />
                        </div>
                        <div class="flex flex-col gap-1">
                          <label class="text-muted-foreground text-xs">{{
                            t("settings.ai.maxTokens")
                          }}</label>
                          <Input
                            v-model="model.maxTokens"
                            class="h-7 text-xs tabular-nums"
                            :placeholder="t('settings.ai.maxTokensPlaceholder')"
                          />
                        </div>
                      </div>
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

    <!-- 添加厂商对话框:先选内置厂商(带入地址与预置模型)或自定义,再补 API Key -->
    <Dialog v-model:open="addDialogOpen">
      <DialogContent class="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{{ t("settings.ai.addProviderTitle") }}</DialogTitle>
          <DialogDescription>{{ t("settings.ai.addProviderDesc") }}</DialogDescription>
        </DialogHeader>

        <div class="flex flex-col gap-3 py-1">
          <div class="flex flex-col gap-1">
            <label class="text-muted-foreground text-xs">{{
              t("settings.ai.providerSelect")
            }}</label>
            <Select :model-value="addChoice" @update:model-value="onAddChoiceChange">
              <SelectTrigger class="h-8 w-full text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectItem :value="CUSTOM_CHOICE">
                    {{ t("settings.ai.customProvider") }}
                  </SelectItem>
                  <SelectItem v-for="option in addOptions" :key="option.id" :value="option.id">
                    {{ option.name }}
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>

          <div class="grid grid-cols-2 gap-2">
            <div class="flex flex-col gap-1">
              <label class="text-muted-foreground text-xs">{{ t("settings.ai.providerId") }}</label>
              <Input
                v-model="addForm.id"
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
                v-model="addForm.name"
                class="h-8 text-xs"
                :placeholder="t('settings.ai.providerNamePlaceholder')"
                spellcheck="false"
              />
            </div>
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-muted-foreground text-xs">{{ t("settings.ai.baseUrl") }}</label>
            <Input
              v-model="addForm.baseUrl"
              class="h-8 text-xs"
              :placeholder="t('settings.ai.baseUrlPlaceholder')"
              spellcheck="false"
            />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-muted-foreground text-xs">{{ t("settings.ai.apiKey") }}</label>
            <Input
              v-model="addForm.apiKey"
              type="password"
              class="h-8 text-xs"
              :placeholder="t('settings.ai.apiKeyPlaceholder')"
              autocomplete="off"
              spellcheck="false"
            />
          </div>
          <p v-if="seededModelCount > 0" class="text-muted-foreground text-xs">
            {{ t("settings.ai.seededModelsHint", { count: seededModelCount }) }}
          </p>
        </div>

        <DialogFooter>
          <Button variant="outline" @click="addDialogOpen = false">{{ t("common.cancel") }}</Button>
          <Button @click="confirmAddProvider">{{ t("common.add") }}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- 从 CC Switch 导入对话框:列出本机 ~/.cc-switch 中 OpenAI chat 兼容的供应商,勾选后并入草稿 -->
    <Dialog v-model:open="ccSwitchDialogOpen">
      <DialogContent class="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{{ t("settings.ai.ccSwitchTitle") }}</DialogTitle>
          <DialogDescription>{{ t("settings.ai.ccSwitchDesc") }}</DialogDescription>
        </DialogHeader>

        <div
          v-if="ccSwitchLoading"
          class="text-muted-foreground flex items-center gap-2 py-6 text-sm"
        >
          <Loader2 class="h-4 w-4 animate-spin" />
          {{ t("settings.ai.ccSwitchLoading") }}
        </div>
        <p
          v-else-if="ccSwitchScan && !ccSwitchScan.found"
          class="text-muted-foreground py-6 text-sm"
        >
          {{ t("settings.ai.ccSwitchNotFound") }}
        </p>
        <p
          v-else-if="ccSwitchScan && !ccSwitchScan.providers.length"
          class="text-muted-foreground py-6 text-sm"
        >
          {{ t("settings.ai.ccSwitchEmpty") }}
        </p>
        <template v-else-if="ccSwitchScan">
          <div class="flex items-center justify-between">
            <span class="text-muted-foreground text-xs">
              {{
                t("settings.ai.ccSwitchSelected", {
                  selected: ccSwitchChecked.size,
                  total: ccSwitchScan.providers.length,
                })
              }}
            </span>
            <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" @click="toggleCcSwitchAll">
              {{
                ccSwitchAllChecked
                  ? t("settings.ai.ccSwitchDeselectAll")
                  : t("settings.ai.ccSwitchSelectAll")
              }}
            </Button>
          </div>
          <div class="flex max-h-72 flex-col gap-1.5 overflow-y-auto py-1">
            <button
              v-for="provider in ccSwitchScan.providers"
              :key="ccSwitchKey(provider)"
              type="button"
              class="hover:bg-accent flex items-center gap-2 rounded-md border px-2 py-1.5 text-left"
              :class="
                ccSwitchChecked.has(ccSwitchKey(provider)) ? 'border-primary/50 bg-primary/5' : ''
              "
              @click="toggleCcSwitchProvider(provider)"
            >
              <span
                class="flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border"
                :class="
                  ccSwitchChecked.has(ccSwitchKey(provider))
                    ? 'border-primary bg-primary text-primary-foreground'
                    : 'border-input'
                "
              >
                <Check v-if="ccSwitchChecked.has(ccSwitchKey(provider))" class="h-3 w-3" />
              </span>
              <span class="flex min-w-0 flex-1 flex-col">
                <span class="flex items-center gap-1.5">
                  <span class="truncate text-sm font-medium">
                    {{ provider.name.trim() || provider.id }}
                  </span>
                  <span
                    class="bg-muted text-muted-foreground shrink-0 rounded-full px-1.5 py-px text-[10px]"
                  >
                    {{ provider.app }}
                  </span>
                  <span
                    v-if="provider.current"
                    class="bg-primary/10 text-primary shrink-0 rounded-full px-1.5 py-px text-[10px]"
                  >
                    {{ t("settings.ai.ccSwitchCurrent") }}
                  </span>
                </span>
                <span class="text-muted-foreground truncate text-xs">{{ provider.baseUrl }}</span>
              </span>
              <span class="text-muted-foreground shrink-0 text-xs">
                {{
                  provider.apiKey
                    ? t("settings.ai.modelCount", { count: provider.models.length })
                    : t("settings.ai.ccSwitchNoKey")
                }}
              </span>
            </button>
          </div>
        </template>

        <DialogFooter>
          <Button variant="outline" @click="ccSwitchDialogOpen = false">
            {{ t("common.cancel") }}
          </Button>
          <Button :disabled="!ccSwitchChecked.size" @click="confirmCcSwitchImport">
            {{ t("settings.ai.ccSwitchImport", { count: ccSwitchChecked.size }) }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </section>
</template>
