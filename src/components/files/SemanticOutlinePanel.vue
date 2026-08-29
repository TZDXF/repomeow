<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { ArrowLeft, Loader2, RefreshCw, Search, Users, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import SemanticEntitySearch from "@/components/files/SemanticEntitySearch.vue";
import SemanticOutlineTree from "@/components/files/SemanticOutlineTree.vue";
import SemanticRelationPanel from "@/components/files/SemanticRelationPanel.vue";
import { useSemanticRequest } from "@/composables/useSemanticRequest";
import {
  buildOutlineTree,
  cacheFileEntities,
  cachedFileEntities,
  compareEntitiesForDisplay,
  entityDisplayName,
  invalidateSemanticCache,
  type SemanticOutlineNode,
} from "@/lib/semantic";
import { cmd } from "@/lib/tauri";
import type {
  SemanticBlameEntry,
  SemanticFileBlameResult,
  SemanticFileEntitiesResult,
  SemanticFileEntity,
} from "@/types";

// 文件结构面板:当前文件实体的树形大纲 + 过滤 + 全项目实体搜索入口 +
// 选中实体的调用者/引用关系。sem 不可用时仅本面板降级,不影响文件树与预览。

const props = defineProps<{
  root: string;
  /** 当前选中文件(仓库内 "/" 分隔相对路径);null 表示无文件 */
  filePath: string | null;
  /** 缓存世代:外部(head 变更等)递增以强制重新加载 */
  cacheEpoch: number;
}>();

const emit = defineEmits<{
  locate: [startLine: number, endLine: number];
  open: [filePath: string, startLine: number, endLine: number];
  impact: [entity: SemanticFileEntity];
  history: [entity: SemanticFileEntity];
}>();

const { t } = useI18n();

// ── 模式:结构大纲 / 全项目实体搜索 ──────────────────────────────────────────
const mode = ref<"outline" | "search">("outline");
const filter = ref("");
const selectedEntity = ref<SemanticFileEntity | null>(null);

const request = useSemanticRequest((requestId: string, file: string) =>
  cmd<SemanticFileEntitiesResult>("semantic_file_entities", {
    path: props.root,
    filePath: file,
    requestId,
  }),
);

const fromCache = ref(false);

async function load(file: string, force = false) {
  selectedEntity.value = null;
  if (!force) {
    const cached = cachedFileEntities(props.root, file);
    if (cached) {
      request.result.value = cached;
      fromCache.value = true;
      return;
    }
  }
  fromCache.value = false;
  const result = await request.run(file);
  if (result) cacheFileEntities(props.root, file, result);
}

watch(
  [() => props.filePath, () => props.cacheEpoch],
  ([file]) => {
    filter.value = "";
    if (file) void load(file, false);
    else request.reset();
  },
  { immediate: true },
);

function refresh() {
  blameMap.value = new Map();
  if (props.filePath) {
    invalidateSemanticCache(props.root, props.filePath);
    void load(props.filePath, true);
  }
}

// ── 负责人标注(sem blame):可选列,首次打开时懒加载整文件 blame ──────────────
const blameOn = ref(false);
const blameMap = ref<Map<string, SemanticBlameEntry>>(new Map());
const blameLoading = ref(false);

const blameRequest = useSemanticRequest((requestId: string, file: string) =>
  cmd<SemanticFileBlameResult>("semantic_file_blame", {
    path: props.root,
    filePath: file,
    requestId,
  }),
);

async function toggleBlame() {
  blameOn.value = !blameOn.value;
  // 首次展开才拉取;切文件后再次展开会重新加载
  if (blameOn.value && !blameMap.value.size && props.filePath) {
    blameLoading.value = true;
    const result = await blameRequest.run(props.filePath);
    blameLoading.value = false;
    if (result) {
      blameMap.value = new Map(
        result.entries.map((entry) => [`${entry.name}:${entry.startLine}`, entry]),
      );
    }
  }
}

watch(
  () => props.filePath,
  () => {
    blameMap.value = new Map();
    blameRequest.reset();
  },
);

// ── 大纲树 / 过滤 ────────────────────────────────────────────────────────────
const tree = computed<SemanticOutlineNode[]>(() =>
  buildOutlineTree(request.result.value?.entities ?? []),
);

/** 过滤时拍平为列表(名称大小写不敏感),按展示序排序 */
const filtered = computed<SemanticFileEntity[]>(() => {
  const q = filter.value.trim().toLowerCase();
  if (!q) return [];
  return (request.result.value?.entities ?? [])
    .filter((entity) => entity.name.toLowerCase().includes(q))
    .sort(compareEntitiesForDisplay);
});

const filtering = computed(() => filter.value.trim().length > 0);

function selectEntity(entity: SemanticFileEntity) {
  selectedEntity.value = entity;
  emit("locate", entity.startLine, entity.endLine);
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <!-- 全项目搜索模式 -->
    <template v-if="mode === 'search'">
      <div class="flex shrink-0 items-center gap-1 border-b px-2 py-1.5">
        <Button
          variant="ghost"
          size="icon"
          class="h-6 w-6"
          :title="t('files.semantic.backToOutline')"
          @click="mode = 'outline'"
        >
          <ArrowLeft class="h-3.5 w-3.5" />
        </Button>
        <span class="text-xs text-muted-foreground">{{ t("files.semantic.searchProject") }}</span>
      </div>
      <SemanticEntitySearch
        class="min-h-0 flex-1"
        :root="root"
        @open="(file, start, end) => emit('open', file, start, end)"
      />
    </template>

    <!-- 结构大纲模式 -->
    <template v-else>
      <div class="flex shrink-0 items-center gap-1 border-b p-2">
        <Input
          v-model="filter"
          :placeholder="t('files.semantic.filterPlaceholder')"
          class="h-7 min-w-0 flex-1 text-xs"
        />
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 shrink-0"
          :title="t('files.semantic.searchProject')"
          @click="mode = 'search'"
        >
          <Search class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 shrink-0"
          :class="blameOn ? 'bg-accent' : ''"
          :title="t('files.semantic.blameToggle')"
          :disabled="blameLoading"
          @click="toggleBlame"
        >
          <Users class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 shrink-0"
          :title="t('common.refresh')"
          @click="refresh"
        >
          <RefreshCw class="h-3.5 w-3.5" />
        </Button>
      </div>

      <ScrollArea class="min-h-0 flex-1">
        <div
          v-if="request.loading.value && !fromCache"
          class="flex items-center gap-1.5 p-3 text-xs text-muted-foreground"
        >
          <Loader2 class="h-3 w-3 animate-spin" />
          {{ t("common.loading") }}
        </div>
        <div v-else-if="request.error.value" class="flex flex-col items-start gap-2 p-3">
          <p class="text-xs text-destructive">{{ request.error.value }}</p>
          <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="refresh">
            <RefreshCw class="h-3 w-3" />
            {{ t("common.retry") }}
          </Button>
        </div>
        <p v-else-if="!filePath" class="p-3 text-xs text-muted-foreground">
          {{ t("files.selectHint") }}
        </p>
        <p v-else-if="!tree.length" class="p-3 text-xs text-muted-foreground">
          {{ t("files.semantic.outlineEmpty") }}
        </p>
        <template v-else>
          <p
            v-if="request.result.value?.truncated"
            class="border-b px-3 py-1.5 text-[11px] text-muted-foreground"
          >
            {{ t("files.semantic.truncated") }}
          </p>
          <!-- 过滤模式:平铺列表 -->
          <div v-if="filtering" class="py-1">
            <p v-if="!filtered.length" class="p-3 text-xs text-muted-foreground">
              {{ t("files.semantic.noMatch") }}
            </p>
            <button
              v-for="entity in filtered"
              :key="entity.entityId ?? `${entity.name}:${entity.startLine}`"
              type="button"
              class="flex w-full items-center gap-1.5 px-3 py-1 text-left hover:bg-accent/60"
              @click="selectEntity(entity)"
            >
              <span class="min-w-0 flex-1 truncate font-mono text-xs">
                {{ entityDisplayName(entity) }}
              </span>
              <span class="shrink-0 text-[10px] text-muted-foreground">{{
                entity.entityType
              }}</span>
              <span class="shrink-0 text-[10px] text-muted-foreground"
                >:{{ entity.startLine }}</span
              >
            </button>
          </div>
          <!-- 树模式 -->
          <SemanticOutlineTree
            v-else
            class="py-1"
            :nodes="tree"
            :depth="0"
            :selected-id="selectedEntity?.entityId ?? null"
            :blame="blameOn ? blameMap : undefined"
            @select="selectEntity"
            @impact="(entity) => emit('impact', entity)"
            @history="(entity) => emit('history', entity)"
          />
        </template>
      </ScrollArea>

      <!-- 选中实体的调用者/引用 -->
      <div v-if="selectedEntity" class="shrink-0 border-t">
        <div class="flex items-center gap-1 px-2 py-1">
          <span
            class="min-w-0 flex-1 truncate font-mono text-[11px]"
            :title="selectedEntity.entityId ?? undefined"
          >
            {{ entityDisplayName(selectedEntity) }}
          </span>
          <Button variant="ghost" size="icon" class="h-5 w-5" @click="selectedEntity = null">
            <X class="h-3 w-3" />
          </Button>
        </div>
        <SemanticRelationPanel
          :root="root"
          :entity="selectedEntity"
          @open="(file, start, end) => emit('open', file, start, end)"
        />
      </div>
    </template>
  </div>
</template>
