<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Check, Loader2, Plug, RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover";
import { fetchAiModels, testAiConnection } from "@/lib/ai";
import { useSettingsStore } from "@/stores/settings";

const { t } = useI18n();
const store = useSettingsStore();

// 本地副本,显式保存后再持久化(API Key 类输入不适合边敲边存)
const baseUrl = ref(store.aiBaseUrl);
const apiKey = ref(store.aiApiKey);
const model = ref(store.aiModel);
const testing = ref(false);

// 模型下拉:拉取到的列表缓存于会话内,输入内容即时过滤
const modelList = ref<string[]>([]);
const fetchingModels = ref(false);
const modelDropdownOpen = ref(false);

const filteredModels = computed(() => {
  const keyword = model.value.trim().toLowerCase();
  if (!keyword) return modelList.value;
  return modelList.value.filter((m) => m.toLowerCase().includes(keyword));
});

async function fetchModels() {
  if (fetchingModels.value) return;
  fetchingModels.value = true;
  try {
    modelList.value = await fetchAiModels(baseUrl.value, apiKey.value);
    modelDropdownOpen.value = modelList.value.length > 0;
    toast.success(t("settings.ai.fetchModelsSuccess", { count: modelList.value.length }));
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    toast.error(t("settings.ai.fetchModelsFailed", { error: message }));
  } finally {
    fetchingModels.value = false;
  }
}

function selectModel(name: string) {
  model.value = name;
  modelDropdownOpen.value = false;
}

// 此处没有 PopoverTrigger,reka 只豁免 trigger 元素的外部交互;
// 输入框就是下拉触发源,聚焦/点击它不算外部交互,否则打开下拉的那次 focusin 冒泡会被判为 focusOutside 导致秒关
function onModelInteractOutside(event: CustomEvent) {
  const target = (event.detail.originalEvent as Event).target as HTMLElement | null;
  if (target?.closest("#ai-model")) event.preventDefault();
}

// AI 调用并发上限(1-5),点选即持久化
const CONCURRENCY_OPTIONS = [1, 2, 3, 4, 5];

async function save() {
  await store.setAiBaseUrl(baseUrl.value);
  await store.setAiApiKey(apiKey.value);
  await store.setAiModel(model.value);
  toast.success(t("settings.ai.saved"));
}

async function testConnection() {
  if (testing.value) return;
  testing.value = true;
  try {
    // 先落库当前表单值,测试使用的就是界面上看到的配置
    await store.setAiBaseUrl(baseUrl.value);
    await store.setAiApiKey(apiKey.value);
    await store.setAiModel(model.value);
    await testAiConnection();
    toast.success(t("settings.ai.testSuccess"));
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    toast.error(t("settings.ai.testFailed", { error: message }));
  } finally {
    testing.value = false;
  }
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.ai.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">{{ t("settings.ai.description") }}</p>

    <div class="mt-4 flex flex-col gap-4">
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium" for="ai-base-url">{{ t("settings.ai.baseUrl") }}</label>
        <Input
          id="ai-base-url"
          v-model="baseUrl"
          :placeholder="t('settings.ai.baseUrlPlaceholder')"
          spellcheck="false"
        />
      </div>
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium" for="ai-api-key">{{ t("settings.ai.apiKey") }}</label>
        <Input
          id="ai-api-key"
          v-model="apiKey"
          type="password"
          :placeholder="t('settings.ai.apiKeyPlaceholder')"
          autocomplete="off"
          spellcheck="false"
        />
      </div>
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium" for="ai-model">{{ t("settings.ai.model") }}</label>
        <div class="flex gap-2">
          <Popover v-model:open="modelDropdownOpen">
            <PopoverAnchor as-child>
              <Input
                id="ai-model"
                v-model="model"
                class="flex-1"
                :placeholder="t('settings.ai.modelPlaceholder')"
                spellcheck="false"
                @focus="modelDropdownOpen = modelList.length > 0"
                @input="modelDropdownOpen = modelList.length > 0"
                @keydown.esc="modelDropdownOpen = false"
              />
            </PopoverAnchor>
            <PopoverContent
              class="w-(--reka-popover-trigger-width) gap-0 p-1"
              align="start"
              @open-auto-focus.prevent
              @close-auto-focus.prevent
              @interact-outside="onModelInteractOutside"
            >
              <div v-if="filteredModels.length" class="max-h-60 overflow-y-auto">
                <button
                  v-for="name in filteredModels"
                  :key="name"
                  type="button"
                  class="hover:bg-accent flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm"
                  @click="selectModel(name)"
                >
                  <Check
                    class="h-3.5 w-3.5 shrink-0"
                    :class="name === model.trim() ? 'opacity-100' : 'opacity-0'"
                  />
                  <span class="truncate">{{ name }}</span>
                </button>
              </div>
              <p v-else class="text-muted-foreground px-2 py-1.5 text-sm">
                {{ t("settings.ai.noMatchingModels") }}
              </p>
            </PopoverContent>
          </Popover>
          <Button
            variant="outline"
            class="shrink-0 gap-1.5"
            :disabled="fetchingModels || !apiKey.trim() || !baseUrl.trim()"
            @click="fetchModels"
          >
            <Loader2 v-if="fetchingModels" class="h-3.5 w-3.5 animate-spin" />
            <RefreshCw v-else class="h-3.5 w-3.5" />
            {{ fetchingModels ? t("settings.ai.fetchingModels") : t("settings.ai.fetchModels") }}
          </Button>
        </div>
      </div>
      <div class="flex items-center gap-2">
        <Button size="sm" @click="save">{{ t("common.save") }}</Button>
        <Button
          size="sm"
          variant="outline"
          class="gap-1.5"
          :disabled="testing || !apiKey.trim() || !baseUrl.trim() || !model.trim()"
          @click="testConnection"
        >
          <Loader2 v-if="testing" class="h-3.5 w-3.5 animate-spin" />
          <Plug v-else class="h-3.5 w-3.5" />
          {{ testing ? t("settings.ai.testing") : t("settings.ai.test") }}
        </Button>
      </div>
      <div class="flex flex-col gap-1.5">
        <label class="text-sm font-medium">{{ t("settings.ai.concurrency") }}</label>
        <div class="flex gap-1.5">
          <button
            v-for="n in CONCURRENCY_OPTIONS"
            :key="n"
            type="button"
            class="h-8 w-8 rounded-md border text-sm transition-colors"
            :class="
              store.aiConcurrency === n
                ? 'border-primary bg-primary/10 font-medium'
                : 'hover:bg-accent'
            "
            @click="store.setAiConcurrency(n)"
          >
            {{ n }}
          </button>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ t("settings.ai.concurrencyHint") }}
        </p>
      </div>
    </div>
  </section>
</template>
