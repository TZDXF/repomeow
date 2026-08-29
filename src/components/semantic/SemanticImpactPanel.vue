<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Loader2, Network, RefreshCw } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import SemanticEntityList from "@/components/semantic/SemanticEntityList.vue";
import SemanticMiniGraph from "@/components/semantic/SemanticMiniGraph.vue";
import { useSemanticRequest } from "@/composables/useSemanticRequest";
import { dedupeEntityRefs } from "@/lib/semantic";
import { cmd, onListen } from "@/lib/tauri";
import type { GitProjectChangedPayload, SemanticEntityRef, SemanticImpactResult } from "@/types";

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

watch([open, () => props.entity, depth], ([isOpen]) => {
  if (isOpen) void load();
  else request.cancel();
});

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
