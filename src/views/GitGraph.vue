<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { ArrowLeft, Copy, GitBranch, Loader2, RefreshCw, Tag as TagIcon, X } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { computeGraphLayout, laneColor, type GraphEdgeLayout } from "@/lib/git-graph";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import type { GitGraphCommit } from "@/types";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const store = useProjectsStore();

const project = computed(() => {
  const id = Number(route.params.id);
  return Number.isFinite(id) ? store.projects.find((p) => p.id === id) : undefined;
});

// 后端默认返回最近 300 条(上限 1000),达到该数量时提示历史被截断
const MAX_COUNT = 300;

const commits = ref<GitGraphCommit[]>([]);
const loading = ref(false);
const loadError = ref("");
const selected = ref<GitGraphCommit | null>(null);

const ROW_H = 32;
const LANE_W = 16;
const GRAPH_PAD = 4;

const layout = computed(() => computeGraphLayout(commits.value));
const graphWidth = computed(() => layout.value.laneCount * LANE_W + GRAPH_PAD * 2);
const graphHeight = computed(() => Math.max(commits.value.length, 0) * ROW_H);

function nodeX(lane: number) {
  return GRAPH_PAD + lane * LANE_W + LANE_W / 2;
}
function nodeY(row: number) {
  return row * ROW_H + ROW_H / 2;
}

/** 连线路径:同泳道直线;跨泳道先在一行高内 S 形换道,再直线落到目标 */
function edgePath(e: GraphEdgeLayout): string {
  const x1 = nodeX(e.fromLane);
  const y1 = nodeY(e.fromRow);
  const x2 = nodeX(e.toLane);
  const y2 = e.toRow >= commits.value.length ? graphHeight.value : nodeY(e.toRow);
  if (x1 === x2) {
    return `M ${x1} ${y1} L ${x2} ${y2}`;
  }
  const bendY = Math.min(y1 + ROW_H, y2);
  return `M ${x1} ${y1} C ${x1} ${y1 + ROW_H * 0.6}, ${x2} ${y1 + ROW_H * 0.4}, ${x2} ${bendY} L ${x2} ${y2}`;
}

async function load() {
  if (!project.value) {
    return;
  }
  loading.value = true;
  loadError.value = "";
  selected.value = null;
  try {
    commits.value = await cmd<GitGraphCommit[]>("git_graph_log", {
      path: project.value.path,
      maxCount: MAX_COUNT,
    });
  } catch (e) {
    loadError.value = String(e);
  } finally {
    loading.value = false;
  }
}

watch(() => project.value?.id, load, { immediate: true });

function toggleSelect(commit: GitGraphCommit) {
  selected.value = selected.value?.hash === commit.hash ? null : commit;
}

function shortHash(hash: string) {
  return hash.slice(0, 7);
}

function isTag(refName: string) {
  return refName.startsWith("tag: ");
}
function tagName(refName: string) {
  return refName.slice(5);
}

async function copyHash(hash: string) {
  await navigator.clipboard.writeText(hash);
  toast.success(t("git.graph.copied"));
}
</script>

<template>
  <div v-if="project" class="flex h-full flex-col">
    <header class="flex items-center gap-3 border-b px-4 py-3">
      <Button variant="ghost" size="sm" @click="router.push(`/projects/${project.id}`)">
        <ArrowLeft class="h-4 w-4" />
        {{ t("git.graph.back") }}
      </Button>
      <div class="min-w-0 flex-1">
        <h1 class="truncate text-sm font-medium">
          {{ project.name }} · {{ t("git.graph.title") }}
        </h1>
      </div>
      <span v-if="commits.length" class="shrink-0 text-xs text-muted-foreground">
        {{ t("git.graph.commitsCount", { count: commits.length }) }}
      </span>
      <Button variant="outline" size="sm" :disabled="loading" @click="load">
        <Loader2 v-if="loading" class="h-3.5 w-3.5 animate-spin" />
        <RefreshCw v-else class="h-3.5 w-3.5" />
        {{ t("git.graph.refresh") }}
      </Button>
    </header>

    <div class="relative flex-1 overflow-auto">
      <div v-if="loading && !commits.length" class="flex h-full items-center justify-center">
        <Loader2 class="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
      <div
        v-else-if="loadError"
        class="flex h-full flex-col items-center justify-center gap-3 text-sm text-muted-foreground"
      >
        <p>{{ t("git.graph.loadFailed") }}:{{ loadError }}</p>
        <Button variant="outline" size="sm" @click="load">{{ t("git.graph.refresh") }}</Button>
      </div>
      <div
        v-else-if="!commits.length"
        class="flex h-full items-center justify-center text-sm text-muted-foreground"
      >
        {{ t("git.graph.empty") }}
      </div>

      <div v-else class="relative" :style="{ height: `${graphHeight}px` }">
        <!-- 泳道连线与节点(SVG 覆盖层) -->
        <svg
          class="pointer-events-none absolute top-0 left-0"
          :width="graphWidth"
          :height="graphHeight"
        >
          <path
            v-for="(e, i) in layout.edges"
            :key="i"
            :d="edgePath(e)"
            :stroke="laneColor(e.color)"
            stroke-width="1.5"
            fill="none"
          />
          <template v-for="n in layout.nodes" :key="n.commit.hash">
            <circle
              v-if="n.commit.is_head"
              :cx="nodeX(n.lane)"
              :cy="nodeY(n.row)"
              r="6.5"
              fill="none"
              :stroke="laneColor(n.color)"
              stroke-width="1.5"
            />
            <circle
              :cx="nodeX(n.lane)"
              :cy="nodeY(n.row)"
              :r="selected?.hash === n.commit.hash ? 5 : 4"
              :fill="laneColor(n.color)"
              class="stroke-background"
              stroke-width="2"
            />
          </template>
        </svg>

        <!-- 提交信息行 -->
        <div
          v-for="n in layout.nodes"
          :key="n.commit.hash"
          class="absolute right-0 left-0 flex cursor-pointer items-center gap-2 pr-4 transition-colors hover:bg-accent/60"
          :class="selected?.hash === n.commit.hash ? 'bg-accent' : ''"
          :style="{
            top: `${n.row * ROW_H}px`,
            height: `${ROW_H}px`,
            paddingLeft: `${graphWidth + 8}px`,
          }"
          @click="toggleSelect(n.commit)"
        >
          <Badge v-if="n.commit.is_head" variant="default" class="h-5 shrink-0 px-1.5 text-[10px]">
            HEAD
          </Badge>
          <template v-for="r in n.commit.refs" :key="r">
            <Badge
              v-if="isTag(r)"
              variant="outline"
              class="h-5 max-w-40 shrink-0 gap-1 px-1.5 text-[10px] text-amber-600 dark:text-amber-400"
            >
              <TagIcon class="h-2.5 w-2.5 shrink-0" />
              <span class="truncate">{{ tagName(r) }}</span>
            </Badge>
            <Badge
              v-else
              variant="secondary"
              class="h-5 max-w-40 shrink-0 gap-1 px-1.5 text-[10px]"
            >
              <GitBranch class="h-2.5 w-2.5 shrink-0" />
              <span class="truncate">{{ r }}</span>
            </Badge>
          </template>
          <span class="min-w-0 flex-1 truncate text-sm">{{ n.commit.subject }}</span>
          <span class="shrink-0 text-xs text-muted-foreground">{{ n.commit.author }}</span>
          <span class="shrink-0 font-mono text-xs text-muted-foreground">
            {{ shortHash(n.commit.hash) }}
          </span>
          <span class="shrink-0 text-xs text-muted-foreground">{{ n.commit.date }}</span>
        </div>
      </div>

      <p
        v-if="commits.length >= MAX_COUNT"
        class="sticky bottom-0 border-t bg-background/95 px-4 py-2 text-center text-xs text-muted-foreground backdrop-blur"
      >
        {{ t("git.graph.truncatedHint", { count: MAX_COUNT }) }}
      </p>
    </div>

    <!-- 底部提交详情面板 -->
    <div v-if="selected" class="shrink-0 border-t bg-muted/30 px-4 py-3">
      <div class="flex items-start justify-between gap-4">
        <div class="min-w-0 flex-1 space-y-1.5 text-sm">
          <p class="font-medium break-all">{{ selected.subject }}</p>
          <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
            <span class="flex items-center gap-1">
              {{ t("git.graph.detail.hash") }}
              <code class="font-mono text-foreground">{{ shortHash(selected.hash) }}</code>
              <button
                class="text-muted-foreground transition-colors hover:text-foreground"
                :title="t('git.graph.copyHash')"
                @click="copyHash(selected.hash)"
              >
                <Copy class="h-3 w-3" />
              </button>
            </span>
            <span>{{ t("git.graph.detail.author") }} {{ selected.author }}</span>
            <span>{{ t("git.graph.detail.date") }} {{ selected.date }}</span>
            <span v-if="selected.parents.length" class="font-mono">
              {{ t("git.graph.detail.parents") }}
              {{ selected.parents.map(shortHash).join(", ") }}
            </span>
          </div>
          <div v-if="selected.refs.length" class="flex flex-wrap gap-1 pt-0.5">
            <Badge
              v-for="r in selected.refs"
              :key="r"
              :variant="isTag(r) ? 'outline' : 'secondary'"
              class="h-5 gap-1 px-1.5 text-[10px]"
            >
              <TagIcon v-if="isTag(r)" class="h-2.5 w-2.5" />
              <GitBranch v-else class="h-2.5 w-2.5" />
              {{ isTag(r) ? tagName(r) : r }}
            </Badge>
          </div>
        </div>
        <Button variant="ghost" size="sm" class="shrink-0" @click="selected = null">
          <X class="h-4 w-4" />
        </Button>
      </div>
    </div>
  </div>

  <div
    v-else
    class="flex h-full flex-col items-center justify-center gap-3 text-sm text-muted-foreground"
  >
    <p>{{ t("projects.detail.notFound") }}</p>
    <Button variant="outline" size="sm" @click="router.push('/')">{{
      t("projects.detail.backToListShort")
    }}</Button>
  </div>
</template>
