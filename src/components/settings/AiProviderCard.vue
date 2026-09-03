<script setup lang="ts">
import { computed, nextTick, reactive, type ComponentPublicInstance } from "vue";
import { useI18n } from "vue-i18n";
import { ChevronDown, Loader2, Plus, RefreshCw, Trash2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
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
import {
  draftModel,
  type ModelCompatDraft,
  type ModelDraft,
  type ProviderDraft,
} from "@/components/settings/ai-provider-draft";

const props = defineProps<{
  draft: ProviderDraft;
  open: boolean;
  /** 两段式删除:父组件确认窗口内为 true */
  confirmDelete: boolean;
  /** 当前默认模型属于本厂商 */
  isDefault: boolean;
  /** 正在拉取模型列表的厂商 key(全局锁,防并发拉取);null = 空闲 */
  fetchingKey: string | null;
  /** 本厂商最近一次「获取模型列表」拉到的模型 ID 候选 */
  fetchedModelIds: string[];
}>();

const emit = defineEmits<{
  "update:open": [open: boolean];
  remove: [];
  fetchModels: [];
}>();

const { t } = useI18n();

const fetching = computed(() => props.fetchingKey === props.draft.key);

// ── 模型行 ───────────────────────────────────────────────────────────

function addModel() {
  props.draft.models = [
    ...props.draft.models,
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

function removeModel(model: ModelDraft) {
  props.draft.models = props.draft.models.filter((item) => item !== model);
}

// ── 模型高级配置(compat 兼容开关) ────────────────────────────────────

/** 各模型行高级配置区的展开开关 */
const advancedOpen = reactive<Record<string, boolean>>({});

/** 暴露给 UI 的兼容开关;maxTokensField 选项为字段名,其余为三态 */
const COMPAT_FIELDS: readonly {
  key: keyof ModelCompatDraft;
  labelKey: string;
  options: readonly string[];
}[] = [
  {
    key: "supportsDeveloperRole",
    labelKey: "settings.ai.compatDeveloperRole",
    options: ["auto", "on", "off"],
  },
  {
    key: "supportsReasoningEffort",
    labelKey: "settings.ai.compatReasoningEffort",
    options: ["auto", "on", "off"],
  },
  {
    key: "supportsStore",
    labelKey: "settings.ai.compatStore",
    options: ["auto", "on", "off"],
  },
  {
    key: "maxTokensField",
    labelKey: "settings.ai.compatMaxTokensField",
    options: ["auto", "max_completion_tokens", "max_tokens"],
  },
];

/** 选项文案:auto/支持/不支持走 i18n,字段名选项原样展示 */
function compatOptionLabel(option: string): string {
  if (option === "auto") return t("settings.ai.compatAuto");
  if (option === "on") return t("settings.ai.compatOn");
  if (option === "off") return t("settings.ai.compatOff");
  return option;
}

function setCompatOption(model: ModelDraft, key: keyof ModelCompatDraft, value: unknown) {
  (model.compat as unknown as Record<string, string>)[key] = String(value);
}

// ── 模型 ID 下拉候选(「获取模型列表」的结果,供填写时挑选,不直接落行) ──

/** 各模型行 ID 输入框的候选弹层开关(仅由行内下拉按钮开合) */
const suggestionOpen = reactive<Record<string, boolean>>({});
/** 弹层顶部搜索框文本(弹层关闭时删除,打开时重置) */
const suggestionQuery = reactive<Record<string, string>>({});
const suggestionSearchRefs = new Map<string, HTMLInputElement>();
const SUGGESTION_LIMIT = 50;

/** 该厂商是否已有拉取到的模型候选(决定模型行是否展示下拉按钮) */
const hasFetchedModels = computed(() => props.fetchedModelIds.length > 0);

/** 当前行的下拉候选:按弹层搜索词模糊过滤、排除其他行已占用的 ID */
function modelSuggestions(model: ModelDraft): string[] {
  const list = props.fetchedModelIds;
  if (!list.length) return [];
  const used = new Set(
    props.draft.models.filter((item) => item !== model).map((item) => item.id.trim()),
  );
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
</script>

<template>
  <Collapsible :open="open" @update:open="emit('update:open', $event)">
    <div class="rounded-lg border">
      <div class="flex items-center gap-2 px-2 py-1.5">
        <CollapsibleTrigger as-child>
          <button
            type="button"
            class="hover:bg-accent flex min-w-0 flex-1 items-center gap-2 rounded-md px-1 py-1 text-left"
          >
            <ChevronDown
              class="text-muted-foreground h-3.5 w-3.5 shrink-0 transition-transform"
              :class="open ? 'rotate-180' : ''"
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
          v-if="isDefault"
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
          :class="confirmDelete ? 'text-destructive' : 'text-muted-foreground'"
          :title="t('settings.ai.deleteProvider')"
          @click="emit('remove')"
        >
          <Trash2 class="h-3.5 w-3.5" />
          {{
            confirmDelete ? t("settings.ai.confirmDeleteProvider") : t("settings.ai.deleteProvider")
          }}
        </Button>
      </div>

      <CollapsibleContent>
        <div class="flex flex-col gap-3 border-t px-3 py-3">
          <div class="grid grid-cols-2 gap-2">
            <div class="flex flex-col gap-1">
              <label class="text-muted-foreground text-xs">{{ t("settings.ai.providerId") }}</label>
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
            <label class="text-muted-foreground text-xs">{{ t("settings.ai.baseUrl") }}</label>
            <Input
              v-model="draft.baseUrl"
              class="h-8 text-xs"
              :placeholder="t('settings.ai.baseUrlPlaceholder')"
              spellcheck="false"
            />
          </div>
          <div class="flex flex-col gap-1">
            <label class="text-muted-foreground text-xs">{{ t("settings.ai.apiKey") }}</label>
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
              <label class="text-muted-foreground text-xs">{{ t("settings.ai.models") }}</label>
              <Button
                variant="outline"
                size="sm"
                class="h-7 gap-1"
                :disabled="fetchingKey !== null || !draft.baseUrl.trim() || !draft.apiKey.trim()"
                :title="t('settings.ai.fetchModelsHint')"
                @click="emit('fetchModels')"
              >
                <Loader2 v-if="fetching" class="h-3 w-3 animate-spin" />
                <RefreshCw v-else class="h-3 w-3" />
                {{ fetching ? t("settings.ai.fetchingModels") : t("settings.ai.fetchModels") }}
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
                    v-if="hasFetchedModels"
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
                    @click="removeModel(model)"
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
                      v-for="option in modelSuggestions(model)"
                      :key="option"
                      type="button"
                      class="hover:bg-accent w-full truncate rounded-sm px-2 py-1 text-left text-xs"
                      @click="pickModelId(model, option)"
                    >
                      {{ option }}
                    </button>
                    <p
                      v-if="!modelSuggestions(model).length"
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
              <button
                type="button"
                class="text-muted-foreground hover:text-foreground flex w-fit items-center gap-1 text-xs"
                @click="advancedOpen[model.key] = !advancedOpen[model.key]"
              >
                <ChevronDown
                  class="h-3 w-3 transition-transform"
                  :class="advancedOpen[model.key] ? 'rotate-180' : ''"
                />
                {{ t("settings.ai.advancedConfig") }}
              </button>
              <div v-if="advancedOpen[model.key]" class="flex flex-col gap-1.5">
                <p class="text-muted-foreground text-xs">
                  {{ t("settings.ai.advancedConfigHint") }}
                </p>
                <div class="grid grid-cols-2 gap-1.5">
                  <div v-for="field in COMPAT_FIELDS" :key="field.key" class="flex flex-col gap-1">
                    <label class="text-muted-foreground text-xs">{{ t(field.labelKey) }}</label>
                    <Select
                      :model-value="model.compat[field.key]"
                      @update:model-value="setCompatOption(model, field.key, $event)"
                    >
                      <SelectTrigger class="h-7 w-full text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          <SelectItem v-for="option in field.options" :key="option" :value="option">
                            {{ compatOptionLabel(option) }}
                          </SelectItem>
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              </div>
            </div>
            <Button variant="outline" size="sm" class="h-7 w-fit gap-1 text-xs" @click="addModel">
              <Plus class="h-3 w-3" />
              {{ t("settings.ai.addModel") }}
            </Button>
          </div>
        </div>
      </CollapsibleContent>
    </div>
  </Collapsible>
</template>
