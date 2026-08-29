<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { ClipboardCopy, Loader2, Network, RefreshCw, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import SemanticEntityList from "@/components/semantic/SemanticEntityList.vue";
import SemanticMiniGraph from "@/components/semantic/SemanticMiniGraph.vue";
import { useSemanticRequest } from "@/composables/useSemanticRequest";
import { buildContextText, dedupeEntityRefs } from "@/lib/semantic";
import { copyToClipboard } from "@/lib/utils";
import { cmd, onListen } from "@/lib/tauri";
import type {
  GitProjectChangedPayload,
  SemanticContextResult,
  SemanticEntityRef,
  SemanticImpactResult,
} from "@/types";

// 影响分析面板(文件页与 GitGraph 实体视图共用):
// sem impact 基于当前工作树/HEAD 的实体图——查看历史提交时须提示「按当前代码结构分析」;
// 已删除实体(semantic_entity_not_found)展示「当前版本已不存在」,不回退同名实体。

const props = defineProps<{
  root: string;
  entity: SemanticEntityRef | null;
  /** 查看的提交不是 HEAD 时显示「按当前代码结构分析」提示 */
  showCurrentCodeNotice?: boolean;
}>();

const emit = defineEmits<{
  /** 跳到源码(由宿主决定:文件页内定位 / 编辑器打开) */
  open: [entity: SemanticEntityRef];
}>();

const open = defineModel<boolean>("open", { required: true });

const { t } = useI18n();

const depth = ref(2);
const tab = ref<"affected" | "tests" | "dependencies" | "dependents">("affected");

// ── 会话缓存:root + entity + depth;HEAD 变化使整个项目的缓存失效并自动重载 ──
const impactCache = new Map<string, SemanticImpactResult>();

function clearCacheForRoot() {
  const prefix = `${props.root}::`;
  for (const key of impactCache.keys()) {
    if (key.startsWith(prefix)) impactCache.delete(key);
  }
}

let unlistenGitChanged: (() => void) | null = null;
onMounted(async () => {
  unlistenGitChanged = await onListen<GitProjectChangedPayload>(
    "git://project-changed",
    (payload) => {
      if (!payload.head_changed || payload.path !== props.root) return;
      clearCacheForRoot();
      if (open.value && props.entity) void load(true);
    },
  );
});
onBeforeUnmount(() => unlistenGitChanged?.());

function cacheKey(): string | null {
  const entity = props.entity;
  if (!entity) return null;
  return `${props.root}::${entity.entityId ?? `${entity.filePath}:${entity.name}`}::${depth.value}`;
}

const request = useSemanticRequest((requestId: string) => {
  const entity = props.entity;
  if (!entity) return Promise.reject(new Error("no entity"));
  return cmd<SemanticImpactResult>("semantic_entity_impact", {
    path: props.root,
    entityId: entity.entityId ?? undefined,
    entityName: entity.entityId ? undefined : entity.name,
    filePath: entity.filePath || undefined,
    depth: depth.value,
    requestId,
  });
});

async function load(force = false) {
  if (!props.entity) return;
  const key = cacheKey()!;
  if (!force) {
    const cached = impactCache.get(key);
    if (cached) {
      request.result.value = cached;
      return;
    }
  }
  const result = await request.run();
  if (result) {
    if (impactCache.size > 100) impactCache.clear();
    impactCache.set(key, result);
  }
}

watch([open, () => props.entity, depth], ([isOpen, entity], [, prevEntity]) => {
  if (entity !== prevEntity) closeContextPreview();
  if (isOpen) void load();
  else {
    request.cancel();
    closeContextPreview();
  }
});

// ── 复制 AI 上下文(第 4B 期):仅用户显式触发;预算/跳数走后端默认(2000/1);
// 先预览将包含的实体与源码规模,确认后才写剪贴板;不落库、不写日志 ──
const contextRequest = useSemanticRequest((requestId: string) => {
  const entity = props.entity;
  if (!entity) return Promise.reject(new Error("no entity"));
  return cmd<SemanticContextResult>("semantic_entity_context", {
    path: props.root,
    entityId: entity.entityId ?? undefined,
    entityName: entity.entityId ? undefined : entity.name,
    filePath: entity.filePath || undefined,
    requestId,
  });
});
const contextPreviewOpen = ref(false);

const contextEntries = computed(() => contextRequest.result.value?.entries ?? []);
const contextOmitted = computed(() => contextRequest.result.value?.omitted ?? []);

function prepareContext() {
  contextPreviewOpen.value = true;
  void contextRequest.run();
}

function closeContextPreview() {
  contextPreviewOpen.value = false;
  contextRequest.reset();
}

function copyContext() {
  const result = contextRequest.result.value;
  if (!result) return;
  const text = buildContextText(result);
  if (!text) return;
  void copyToClipboard(text);
  closeContextPreview();
}

const entityMissing = computed(() => request.errorCode.value === "semantic_entity_not_found");

const affected = computed(() => request.result.value?.affected ?? []);
const tests = computed(() => dedupeEntityRefs(request.result.value?.tests ?? []));
const dependencies = computed(() => request.result.value?.dependencies ?? []);
const dependents = computed(() => request.result.value?.dependents ?? []);

// ── 关系小图:只画 target + 直接依赖/调用者;两侧都为空时无图可画,禁用开关 ──
const showGraph = ref(true);
const hasDirectRelations = computed(() => dependencies.value.length + dependents.value.length > 0);

const tabs = computed(() => {
  const r = request.result.value;
  return [
    { key: "affected" as const, count: r?.total ?? 0 },
    { key: "tests" as const, count: tests.value.length },
    { key: "dependencies" as const, count: dependencies.value.length },
    { key: "dependents" as const, count: dependents.value.length },
  ];
});
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[85vh] flex-col sm:max-w-2xl">
      <DialogHeader class="shrink-0">
        <DialogTitle class="flex items-center gap-2 pr-8 text-sm">
          <span class="min-w-0 flex-1 truncate font-mono" :title="entity?.entityId ?? undefined">
            {{ entity?.name }}
          </span>
          <span v-if="entity" class="shrink-0 text-xs font-normal text-muted-foreground">
            {{ entity.entityType }} · {{ entity.filePath }}:{{ entity.startLine }}
          </span>
        </DialogTitle>
      </DialogHeader>

      <div class="flex shrink-0 items-center gap-2 border-b pb-2">
        <span class="text-xs text-muted-foreground">{{ t("files.semantic.impactDepth") }}</span>
        <div class="flex items-center gap-0.5">
          <Button
            v-for="n in [1, 2, 3, 4, 5]"
            :key="n"
            variant="ghost"
            size="sm"
            class="h-6 w-7 px-0 text-xs"
            :class="depth === n ? 'bg-accent' : 'text-muted-foreground'"
            @click="depth = n"
          >
            {{ n }}
          </Button>
        </div>
        <div class="flex-1" />
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :title="t('files.semantic.copyContextHint')"
          @click="prepareContext"
        >
          <ClipboardCopy class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :class="showGraph && hasDirectRelations ? '' : 'text-muted-foreground'"
          :disabled="!hasDirectRelations"
          :title="t('files.semantic.graphToggle')"
          @click="showGraph = !showGraph"
        >
          <Network class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          :title="t('common.refresh')"
          @click="load(true)"
        >
          <RefreshCw class="h-3.5 w-3.5" />
        </Button>
      </div>

      <p
        v-if="showCurrentCodeNotice"
        class="shrink-0 rounded-md bg-muted px-3 py-1.5 text-xs text-muted-foreground"
      >
        {{ t("files.semantic.impactCurrentCode") }}
      </p>

      <!-- 复制 AI 上下文预览:列出将包含的实体/文件/tokens,确认后才写剪贴板 -->
      <div v-if="contextPreviewOpen" class="shrink-0 space-y-2 rounded-md border p-2">
        <div class="flex items-center gap-2">
          <span class="text-xs font-medium">{{ t("files.semantic.copyContext") }}</span>
          <Loader2
            v-if="contextRequest.loading.value"
            class="h-3 w-3 animate-spin text-muted-foreground"
          />
          <div class="flex-1" />
          <button
            type="button"
            class="rounded-sm p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
            :title="t('common.cancel')"
            @click="closeContextPreview"
          >
            <X class="h-3.5 w-3.5" />
          </button>
        </div>
        <p
          v-if="contextRequest.loading.value"
          class="text-xs text-muted-foreground"
        >
          {{ t("common.loading") }}
        </p>
        <p v-else-if="contextRequest.error.value" class="text-xs text-destructive">
          {{ contextRequest.error.value }}
        </p>
        <template v-else-if="contextRequest.result.value">
          <p v-if="!contextEntries.length" class="text-xs text-muted-foreground">
            {{ t("files.semantic.contextEmpty") }}
          </p>
          <template v-else>
            <p class="text-xs text-muted-foreground">
              {{
                t("files.semantic.contextSummary", {
                  count: contextEntries.length,
                  tokens: contextRequest.result.value.totalTokens,
                })
              }}
            </p>
            <ScrollArea class="max-h-36">
              <div
                v-for="entry in contextEntries"
                :key="entry.entityId || `${entry.filePath}:${entry.name}`"
                class="flex items-center gap-1.5 py-0.5 text-xs"
              >
                <span class="min-w-0 truncate font-mono">{{ entry.name }}</span>
                <span class="shrink-0 text-muted-foreground">
                  {{ entry.entityType }} · {{ entry.role }}
                </span>
                <span class="min-w-0 flex-1 truncate text-muted-foreground" :title="entry.filePath">
                  {{ entry.filePath }}
                </span>
                <span class="shrink-0 text-muted-foreground">~{{ entry.tokens }}</span>
              </div>
            </ScrollArea>
            <p
              v-for="group in contextOmitted"
              :key="group.role"
              class="text-[11px] text-muted-foreground"
            >
              {{
                t("files.semantic.contextOmittedGroup", {
                  entities: group.entities,
                  tests: group.tests,
                  role: group.role,
                })
              }}
            </p>
            <p
              v-if="contextRequest.result.value.targetOmitted"
              class="text-[11px] text-muted-foreground"
            >
              {{ t("files.semantic.contextTargetOmitted") }}
            </p>
            <div class="flex justify-end gap-1.5">
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                @click="closeContextPreview"
              >
                {{ t("common.cancel") }}
              </Button>
              <Button size="sm" class="h-7 gap-1.5 px-2 text-xs" @click="copyContext">
                <ClipboardCopy class="h-3 w-3" />
                {{ t("files.semantic.copyConfirm") }}
              </Button>
            </div>
          </template>
        </template>
      </div>

      <div
        v-if="request.loading.value"
        class="flex flex-1 items-center justify-center gap-2 py-10 text-sm text-muted-foreground"
      >
        <Loader2 class="h-4 w-4 animate-spin" />
        {{ t("common.loading") }}
      </div>

      <p
        v-else-if="entityMissing"
        class="flex-1 px-1 py-10 text-center text-sm text-muted-foreground"
      >
        {{ t("files.semantic.impactEntityGone") }}
      </p>

      <div v-else-if="request.error.value" class="flex flex-1 flex-col items-center gap-2 py-10">
        <p class="whitespace-pre-line text-xs text-destructive">{{ request.error.value }}</p>
        <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="load(true)">
          <RefreshCw class="h-3 w-3" />
          {{ t("common.retry") }}
        </Button>
      </div>

      <template v-else-if="request.result.value">
        <!-- 摘要卡 -->
        <div class="grid shrink-0 grid-cols-4 gap-2">
          <div
            v-for="card in tabs"
            :key="card.key"
            class="rounded-md border px-2 py-1.5 text-center"
          >
            <div class="text-sm font-semibold">{{ card.count }}</div>
            <div class="text-[10px] text-muted-foreground">
              {{ t(`files.semantic.impactTab.${card.key}`) }}
            </div>
          </div>
        </div>

        <!-- 关系小图:target + 直接依赖/调用者(sem 实际返回的边) -->
        <div v-if="showGraph && hasDirectRelations" class="shrink-0 rounded-md border px-2 py-1">
          <SemanticMiniGraph
            :target="request.result.value.entity"
            :dependencies="dependencies"
            :dependents="dependents"
            @open="(e) => emit('open', e)"
          />
        </div>

        <div class="flex shrink-0 items-center gap-1 border-b">
          <Button
            v-for="card in tabs"
            :key="card.key"
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs"
            :class="tab === card.key ? 'bg-accent' : 'text-muted-foreground'"
            @click="tab = card.key"
          >
            {{ t(`files.semantic.impactTab.${card.key}`) }} ({{ card.count }})
          </Button>
        </div>

        <p
          v-if="request.result.value.truncated"
          class="shrink-0 px-1 py-1 text-[11px] text-muted-foreground"
        >
          {{ t("files.semantic.truncated") }}
        </p>

        <ScrollArea class="min-h-0 flex-1">
          <SemanticEntityList
            v-if="tab === 'affected'"
            :items="affected"
            show-depth
            :empty-text="t('files.semantic.impactEmpty')"
            @open="(e) => emit('open', e)"
          />
          <SemanticEntityList
            v-else-if="tab === 'tests'"
            :items="tests"
            :empty-text="t('files.semantic.impactNoTests')"
            @open="(e) => emit('open', e)"
          />
          <SemanticEntityList
            v-else-if="tab === 'dependencies'"
            :items="dependencies"
            :empty-text="t('files.semantic.impactEmpty')"
            @open="(e) => emit('open', e)"
          />
          <SemanticEntityList
            v-else
            :items="dependents"
            :empty-text="t('files.semantic.impactEmpty')"
            @open="(e) => emit('open', e)"
          />
        </ScrollArea>
      </template>
    </DialogContent>
  </Dialog>
</template>
