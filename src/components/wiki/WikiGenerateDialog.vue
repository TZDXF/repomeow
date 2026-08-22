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
import { acpTestCached, agentList, type AcpTestResult } from "@/lib/agent";
import { useSettingsStore } from "@/stores/settings";

/**
 * Wiki 生成配置对话框:点「生成/重新生成」(generate 模式)或 wiki 页右上角
 * 配置入口(edit 模式)时打开,选择后端(内置 API / 已安装的精选 agent)与
 * agent 的模型、思考强度。打开时预填上次使用的配置(持久化在全局设置),
 * 确认才写回;取消则丢弃改动。agent 选中后自动探测其上报的模型/思考强度清单
 * (acpTestCached 应用会话级缓存,不重复 spawn)。是否随之触发生成由调用方决定。
 */
const props = defineProps<{
  /** false 表示关闭 */
  open: boolean;
  /** generate = 确认按钮为「开始生成」;edit = 仅保存配置,按钮为「保存」 */
  mode?: "generate" | "edit";
}>();
const emit = defineEmits<{ close: []; confirm: [] }>();

const { t } = useI18n();
const store = useSettingsStore();

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
onMounted(() => {
  agentList()
    .then((list) => {
      installedAgents.value = list.filter((a) => a.installed);
      // 清单晚于打开到达时,补一次归一(存储值可能指向已卸载的 agent)
      backend.value = normalizeBackend(backend.value);
    })
    .catch(() => {});
});

// ── 本地副本:打开时从设置同步,确认才写回 ─────────────────────────────────

const backend = ref("builtin");
const model = ref("");
const thinking = ref("");

/** 归一后端选择:仅内置与已安装的精选 agent 合法,其余(含历史遗留的自定义)回退内置 */
function normalizeBackend(value: string): string {
  return value !== "builtin" && installedAgents.value.some((a) => a.id === value)
    ? value
    : "builtin";
}

watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) {
      return;
    }
    backend.value = normalizeBackend(store.wikiGenBackend);
    model.value = store.wikiAgentModel;
    thinking.value = store.wikiAgentThinking;
    // 已选中 agent 后端时立即自动获取模型清单(命中缓存则零开销)
    void probeAgent();
  },
);

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

const probeKey = computed(() => backend.value);

const probeState = ref<{
  key: string;
  loading: boolean;
  failed: boolean;
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
  probeState.value = { key, loading: true, failed: false, result: null };
  try {
    // acpTestCached 命中会话缓存时立即返回,不会重复 spawn
    const result = await acpTestCached(key, { agentId: backend.value }, force);
    if (probeKey.value === key) {
      probeState.value = { key, loading: false, failed: false, result };
    }
  } catch {
    if (probeKey.value === key) {
      probeState.value = { key, loading: false, failed: true, result: null };
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
    return t("wiki.agentFetchModelsFailed");
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
  await store.setWikiGenBackend(backend.value);
  await store.setWikiAgentModel(model.value);
  await store.setWikiAgentThinking(thinking.value);
  emit("confirm");
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
        <!-- 后端:内置 API + 已安装的精选 agent(未安装/自定义不展示) -->
        <div class="flex flex-col gap-1.5">
          <label class="text-sm font-medium">{{ t("wiki.genBackend") }}</label>
          <Select :model-value="backend" @update:model-value="onBackendChange">
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

        <!-- 模型 / 思考强度 -->
        <template v-if="backend !== 'builtin'">
          <div class="grid gap-3">
            <div class="flex min-w-0 flex-col gap-1.5">
              <label class="text-sm font-medium">{{ t("wiki.agentModel") }}</label>
              <Select
                :model-value="model || DEFAULT_VALUE"
                :disabled="probeLoading"
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
                :disabled="probeLoading || thinkingChoices.length === 0"
                @update:model-value="onThinkingChange"
              >
                <SelectTrigger class="min-w-0 w-full">
                  <SelectValue class="min-w-0 flex-1 truncate text-left" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem :value="DEFAULT_VALUE">
                    {{ t("wiki.agentThinkingDefault") }}
                  </SelectItem>
                  <SelectItem v-for="c in thinkingOptions" :key="c.id" :value="c.id">
                    {{ c.name }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <p
            v-if="probeHint"
            class="flex items-center gap-1.5 text-xs"
            :class="probeFailed ? 'text-amber-600 dark:text-amber-400' : 'text-muted-foreground'"
          >
            <Loader2 v-if="probeLoading" class="h-3 w-3 animate-spin" />
            {{ probeHint }}
          </p>
        </template>
      </div>

      <div class="flex shrink-0 justify-end gap-2 pt-2">
        <Button variant="outline" size="sm" @click="emit('close')">
          {{ t("common.cancel") }}
        </Button>
        <Button size="sm" @click="confirm">
          {{ confirmLabel }}
        </Button>
      </div>
    </DialogContent>
  </Dialog>
</template>
