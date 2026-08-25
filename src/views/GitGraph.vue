<script setup lang="ts">
import { computed, ref, shallowRef, triggerRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { Channel } from "@tauri-apps/api/core";
import { useElementSize, useLocalStorage, useVirtualList } from "@vueuse/core";
import {
  ArrowLeft,
  ChevronDown,
  GitBranch,
  ListFilter,
  Loader2,
  PanelRightOpen,
  RefreshCw,
  Search,
  Tag as TagIcon,
  X,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  createGraphLayouter,
  laneColor,
  type GraphEdgeLayout,
  type GraphNodeLayout,
} from "@/lib/git-graph";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import {
  useGitGraphColumnSizing,
  useGitGraphDetailSizing,
} from "@/composables/git/useGitGraphSizing";
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import CommitDetailPanel from "@/components/git/CommitDetailPanel.vue";
import GitBranchDeleteDialog from "@/components/git/GitBranchDeleteDialog.vue";
import GitGraphSidebar from "@/components/git/GitGraphSidebar.vue";
import type { GitBranches, GitGraphCommit } from "@/types";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const store = useProjectsStore();

const project = computed(() => {
  const id = Number(route.params.id);
  return Number.isFinite(id) ? store.projects.find((p) => p.id === id) : undefined;
});

/** git_graph_log 流式批次(done = true 表示提交序列结束) */
interface GitGraphBatch {
  commits: GitGraphCommit[];
  done: boolean;
}

const branches = ref<GitBranches>({ local: [], remote: [], tracking: [] });
const loading = ref(false);
const loadError = ref("");
const streamDone = ref(false);
const selected = ref<GitGraphCommit | null>(null);

// 分支筛选:"all" 显示所有分支;"current" 仅显示左侧列表中选中的分支(未选择时回退到当前检出的分支)
const filterValue = ref("all");
const selectedBranch = ref("");
// 搜索:匹配提交信息 / hash / 作者
const searchQuery = ref("");

const currentBranch = computed(() => project.value?.git?.branch ?? "");

const ROW_H = 32;
const LANE_W = 16;
const GRAPH_PAD = 4;
const NODE_R = 4;
const HEADER_H = 32;
// 流式批次大小(后端钳制 50..2000)
const BATCH_SIZE = 500;

// --- 增量泳道布局:push 就地追加,批次到达后 triggerRef 触发视图更新 ---
const layouter = createGraphLayouter();
const nodes = shallowRef<GraphNodeLayout[]>(layouter.nodes);
const edges = shallowRef<GraphEdgeLayout[]>([]);
const laneCount = ref(1);
const totalCount = computed(() => nodes.value.length);

/** 图形区自然宽度:泳道间距固定,空间不足时表格横向滚动或裁剪图谱列,而不是压缩泳道 */
const graphWidth = computed(() => laneCount.value * LANE_W + GRAPH_PAD * 2);

function nodeX(lane: number) {
  return GRAPH_PAD + lane * LANE_W + LANE_W / 2;
}
function nodeY(row: number) {
  return row * ROW_H + ROW_H / 2;
}

// --- 虚拟列表:只渲染可视窗口内的行与连线 ---
// 不用 vueuse 的 wrapperProps(它按"流式行 + marginTop 偏移"设计);
// 本页行与 SVG 均按 row * ROW_H 绝对定位,容器用全高占位
const {
  list: visibleNodes,
  containerProps,
  scrollTo,
} = useVirtualList(nodes, {
  itemHeight: ROW_H,
  overscan: 10,
});

const startIndex = computed(() => visibleNodes.value[0]?.index ?? 0);
const endIndex = computed(() => visibleNodes.value[visibleNodes.value.length - 1]?.index ?? 0);

const { width: containerWidth } = useElementSize(containerProps.ref);
const { colWidths, graphColWidth, graphClipPath, descColWidth, totalWidth, startColResize } =
  useGitGraphColumnSizing(containerWidth, graphWidth);

/** 可视窗口内的连线:edges 按 fromRow 升序,二分截断后按 toRow 过滤(保留穿越窗口的长线) */
const visibleEdges = computed(() => {
  const all = edges.value;
  const bottom = endIndex.value;
  const top = startIndex.value;
  let lo = 0;
  let hi = all.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (all[mid].fromRow <= bottom) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  const out: GraphEdgeLayout[] = [];
  for (let i = 0; i < lo; i++) {
    if (all[i].toRow >= top) {
      out.push(all[i]);
    }
  }
  return out;
});

/** SVG 画布顶部对应的像素偏移(连线/节点坐标需减去该值) */
const svgOffsetY = computed(() => startIndex.value * ROW_H);

function nodeYRel(row: number) {
  return nodeY(row) - svgOffsetY.value;
}

/** 连线路径:同泳道直线;跨泳道先在一行高内 S 形换道,再直线落到目标 */
function edgePath(e: GraphEdgeLayout): string {
  const x1 = nodeX(e.fromLane);
  const y1 = nodeYRel(e.fromRow);
  const x2 = nodeX(e.toLane);
  const y2 = nodeYRel(e.toRow);
  if (x1 === x2) {
    return `M ${x1} ${y1} L ${x2} ${y2}`;
  }
  const bendY = Math.min(y1 + ROW_H, y2);
  return `M ${x1} ${y1} C ${x1} ${y1 + ROW_H * 0.6}, ${x2} ${y1 + ROW_H * 0.4}, ${x2} ${bendY} L ${x2} ${y2}`;
}

// --- 分支筛选 ---
/** 筛选目标分支:左侧选中的分支,未选择时回退到当前检出的分支 */
const resolvedFilterBranch = computed(() => selectedBranch.value || currentBranch.value);

/** 后端修订范围参数:"current" 且有目标分支时按分支取日志,否则 --all 含远程 */
const graphParams = computed(() => {
  if (filterValue.value === "current" && resolvedFilterBranch.value) {
    return { branches: [resolvedFilterBranch.value] as string[] | null };
  }
  return { branches: null as string[] | null };
});

const filterLabel = computed(() => {
  if (filterValue.value === "current") {
    return resolvedFilterBranch.value || t("git.graph.filterCurrent");
  }
  return t("git.graph.filterAll");
});

// --- 标签(从已加载提交的引用装饰中收集) ---
const tags = computed(() => {
  const seen: string[] = [];
  for (const n of nodes.value) {
    for (const r of n.commit.refs) {
      if (isTag(r)) {
        const name = tagName(r);
        if (!seen.includes(name)) {
          seen.push(name);
        }
      }
    }
  }
  return seen;
});

const hasSidebar = computed(
  () =>
    branches.value.local.length > 0 || branches.value.remote.length > 0 || tags.value.length > 0,
);

/** 左侧分支/标签列表整体折叠(持久化) */
const sidebarOpen = useLocalStorage("repomeow:graph-sidebar-open", true);
const mainRowEl = ref<HTMLElement | null>(null);
const { detailOpen, effectiveDetailWidth, startDetailResize } = useGitGraphDetailSizing(
  hasSidebar,
  sidebarOpen,
  mainRowEl,
);

// --- 搜索 ---
const searchResults = computed<GitGraphCommit[]>(() => {
  const q = searchQuery.value.trim().toLowerCase();
  if (!q) {
    return [];
  }
  const out: GitGraphCommit[] = [];
  for (const n of nodes.value) {
    const c = n.commit;
    if (
      c.subject.toLowerCase().includes(q) ||
      c.hash.toLowerCase().includes(q) ||
      c.author.toLowerCase().includes(q)
    ) {
      out.push(c);
      if (out.length >= 20) {
        break;
      }
    }
  }
  return out;
});
const matchHashes = computed(() => new Set(searchResults.value.map((c) => c.hash)));

// --- 流式加载 ---
// 递增序号使过期流的批次被丢弃(切换筛选/项目时旧流不污染新数据)
let loadSeq = 0;

function appendBatch(batch: GitGraphCommit[]) {
  if (!batch.length) {
    return;
  }
  layouter.push(batch);
  triggerRef(nodes);
  edges.value = layouter.edges();
  laneCount.value = layouter.laneCount;
}

async function load() {
  if (!project.value) {
    return;
  }
  const seq = ++loadSeq;
  loading.value = true;
  loadError.value = "";
  streamDone.value = false;
  selected.value = null;
  layouter.reset();
  nodes.value = layouter.nodes;
  edges.value = [];
  laneCount.value = layouter.laneCount;
  let autoLocated = false;
  try {
    const channel = new Channel<GitGraphBatch>();
    channel.onmessage = (batch) => {
      if (seq !== loadSeq) {
        return;
      }
      appendBatch(batch.commits);
      // 单分支筛选模式下自动定位到目标分支的顶端提交
      if (!autoLocated && filterValue.value === "current" && selectedBranch.value) {
        const tip = layouter.nodes.find((n) => n.commit.refs.includes(selectedBranch.value));
        if (tip) {
          autoLocated = true;
          locateNode(tip);
        }
      }
      if (batch.done) {
        streamDone.value = true;
        loading.value = false;
      }
    };
    const [branchData] = await Promise.all([
      cmd<GitBranches>("list_git_branches", { path: project.value.path }),
      cmd("git_graph_log", {
        path: project.value.path,
        ...graphParams.value,
        batchSize: BATCH_SIZE,
        onBatch: channel,
      }),
    ]);
    branches.value = branchData;
  } catch (e) {
    if (seq === loadSeq) {
      loadError.value = String(e);
      loading.value = false;
    }
  }
}

// 切换项目时重置筛选与选择状态
watch(
  () => project.value?.id,
  () => {
    selectedBranch.value = "";
    filterValue.value = "all";
    searchQuery.value = "";
  },
);

watch([() => project.value?.id, filterValue], load, { immediate: true });

// 筛选模式下切换左侧选中分支需要重新拉取该分支的日志
watch(selectedBranch, () => {
  if (filterValue.value === "current") {
    load();
  }
});

// --- 定位 ---
function locateNode(node: GraphNodeLayout) {
  selected.value = node.commit;
  const el = containerProps.ref.value;
  const viewRows = el ? Math.max(Math.floor(el.clientHeight / ROW_H), 1) : 10;
  scrollTo(Math.max(0, node.row - Math.floor(viewRows / 2)));
}

function locateCommit(commit: GitGraphCommit) {
  const node = layouter.nodes.find((n) => n.commit.hash === commit.hash);
  if (node) {
    locateNode(node);
  }
}

/** 定位分支顶端提交;尚未加载到时提示 */
function locateRef(name: string) {
  const node = layouter.nodes.find((n) => n.commit.refs.includes(name));
  if (node) {
    locateNode(node);
  } else {
    toast.info(t("git.graph.tipNotFound"));
  }
}

function locateTag(tag: string) {
  const node = layouter.nodes.find((n) => n.commit.refs.some((r) => r === `tag: ${tag}`));
  if (node) {
    locateNode(node);
  }
}

/** 左侧点击分支:记为筛选目标分支;非筛选模式下定位其顶端提交(筛选模式由流式加载自动定位) */
function selectBranch(name: string) {
  selectedBranch.value = name;
  if (filterValue.value !== "current") {
    locateRef(name);
  }
}

// --- 本地分支右键:拉取 / 推送(非当前分支由后端快进更新或直接推送,不切换工作区) ---
const branchOp = ref<{ branch: string; op: "pull" | "push" } | null>(null);
const conflictOpen = ref(false);
const conflictFiles = ref<string[]>([]);

async function pullBranch(name: string): Promise<boolean> {
  const p = project.value;
  if (!p || branchOp.value) {
    return false;
  }
  branchOp.value = { branch: name, op: "pull" };
  try {
    const conflicts = await store.pullRepository(p, name);
    if (conflicts.length) {
      // 仅当前分支的 pull 可能产生合并冲突:引导用户在编辑器/终端中解决
      conflictFiles.value = conflicts;
      conflictOpen.value = true;
      return false;
    }
    toast.success(t("git.pull.success"));
    load();
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  } finally {
    branchOp.value = null;
  }
}

async function pushBranch(name: string) {
  const p = project.value;
  if (!p || branchOp.value) {
    return;
  }
  branchOp.value = { branch: name, op: "push" };
  try {
    await store.pushRepository(p, name);
    toast.success(t("git.push.success"));
    load();
  } catch (e) {
    const code = (e as Error & { code?: string }).code;
    if (code === "git_push_rejected") {
      // 远端有本地缺失的提交:给出拉取并推送的快捷修复入口
      toast.error(t("git.push.rejected"), {
        action: { label: t("git.push.pullAndPush"), onClick: () => pullThenPushBranch(name) },
      });
    } else {
      toast.error(String(e));
    }
  } finally {
    branchOp.value = null;
  }
}

/** 先拉取;无冲突则自动重试推送 */
async function pullThenPushBranch(name: string) {
  if (await pullBranch(name)) {
    await pushBranch(name);
  }
}

// --- 删除本地分支:先 -d 安全删除;未合并时对话框切换为强制删除确认(-D) ---
const deleteOpen = ref(false);
const deleteTarget = ref("");
const deleteNeedsForce = ref(false);
const deleting = ref(false);

function askDeleteBranch(name: string) {
  deleteTarget.value = name;
  deleteNeedsForce.value = false;
  deleteOpen.value = true;
}

async function confirmDeleteBranch() {
  const p = project.value;
  const name = deleteTarget.value;
  if (!p || !name || deleting.value) {
    return;
  }
  deleting.value = true;
  try {
    await store.deleteBranch(p, name, deleteNeedsForce.value);
    toast.success(t("git.branch.deleted", { name }));
    deleteOpen.value = false;
    // 被删分支恰为筛选目标时清除选中,避免触发对已删分支的重新拉取
    if (selectedBranch.value === name) {
      selectedBranch.value = "";
    }
    load();
  } catch (e) {
    const code = (e as Error & { code?: string }).code;
    if (code === "git_branch_not_merged" && !deleteNeedsForce.value) {
      deleteNeedsForce.value = true;
    } else {
      toast.error(String(e));
      deleteOpen.value = false;
    }
  } finally {
    deleting.value = false;
  }
}

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
</script>

<template>
  <div v-if="project" class="flex h-full flex-col">
    <header class="flex items-center gap-2 border-b px-4 py-3">
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8 shrink-0"
        :title="t('git.graph.back')"
        @click="router.push(`/projects/${project.id}`)"
      >
        <ArrowLeft class="h-4 w-4" />
      </Button>
      <div class="min-w-0 flex-1">
        <h1 class="truncate text-sm font-medium">
          {{ project.name }} · {{ t("git.graph.title") }}
        </h1>
      </div>

      <!-- 搜索:提交信息 / hash / 作者 -->
      <div class="relative w-60 shrink-0">
        <Search
          class="absolute top-1/2 left-2.5 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="searchQuery"
          :placeholder="t('git.graph.searchPlaceholder')"
          class="h-8 pr-7 pl-8 text-xs"
          @keydown.enter.prevent="searchResults[0] && locateCommit(searchResults[0])"
          @keydown.esc="searchQuery = ''"
        />
        <button
          v-if="searchQuery"
          class="absolute top-1/2 right-2 -translate-y-1/2 text-muted-foreground transition-colors hover:text-foreground"
          @click="searchQuery = ''"
        >
          <X class="h-3.5 w-3.5" />
        </button>
        <div
          v-if="searchQuery.trim()"
          class="absolute top-full right-0 z-50 mt-1 max-h-72 w-80 overflow-auto rounded-md border bg-popover p-1 shadow-md"
        >
          <p v-if="!searchResults.length" class="px-2 py-1.5 text-xs text-muted-foreground">
            {{ t("git.graph.searchEmpty") }}
          </p>
          <button
            v-for="c in searchResults"
            :key="c.hash"
            class="flex w-full flex-col gap-0.5 rounded-sm px-2 py-1.5 text-left transition-colors hover:bg-accent"
            @click="locateCommit(c)"
          >
            <span class="truncate text-xs">{{ c.subject }}</span>
            <span class="truncate font-mono text-[10px] text-muted-foreground">
              {{ shortHash(c.hash) }} · {{ c.author }} · {{ c.date }}
            </span>
          </button>
        </div>
      </div>

      <!-- 分支筛选:所有分支 / 当前分支(左侧选中的分支) -->
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="outline" size="sm" class="max-w-48 shrink-0 gap-1">
            <ListFilter class="h-3.5 w-3.5 shrink-0" />
            <span class="truncate">{{ filterLabel }}</span>
            <ChevronDown class="h-3 w-3 shrink-0 opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-56">
          <DropdownMenuRadioGroup v-model="filterValue">
            <DropdownMenuRadioItem value="all">
              {{ t("git.graph.filterAll") }}
            </DropdownMenuRadioItem>
            <DropdownMenuRadioItem value="current" :disabled="!resolvedFilterBranch">
              {{ t("git.graph.filterCurrent") }}
              <span v-if="resolvedFilterBranch" class="ml-1 truncate text-muted-foreground">
                ({{ resolvedFilterBranch }})
              </span>
            </DropdownMenuRadioItem>
          </DropdownMenuRadioGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      <span v-if="totalCount" class="shrink-0 text-xs text-muted-foreground">
        {{ t("git.graph.commitsCount", { count: totalCount }) }}
      </span>
      <Button variant="outline" size="sm" class="shrink-0" :disabled="loading" @click="load">
        <Loader2 v-if="loading" class="h-3.5 w-3.5 animate-spin" />
        <RefreshCw v-else class="h-3.5 w-3.5" />
        {{ t("git.graph.refresh") }}
      </Button>
    </header>

    <div ref="mainRowEl" class="flex min-h-0 flex-1">
      <GitGraphSidebar
        v-if="hasSidebar"
        v-model:open="sidebarOpen"
        :branches="branches"
        :tags="tags"
        :current-branch="currentBranch"
        :selected-branch="selectedBranch"
        :branch-op="branchOp"
        @select-branch="selectBranch"
        @locate-tag="locateTag"
        @pull-branch="pullBranch"
        @push-branch="pushBranch"
        @delete-branch="askDeleteBranch"
      />

      <div v-bind="containerProps" class="relative flex-1 overflow-auto">
        <div v-if="loading && !totalCount" class="flex h-full items-center justify-center">
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
          v-else-if="streamDone && !totalCount"
          class="flex h-full items-center justify-center text-sm text-muted-foreground"
        >
          {{ t("git.graph.empty") }}
        </div>

        <template v-else>
          <!-- 表头:列名 + 拖拽分隔条调整列宽(图谱列可拖窄,图形裁剪而非压缩) -->
          <div
            class="sticky top-0 z-20 flex items-center border-b bg-background text-xs font-medium text-muted-foreground"
            :style="{ width: `${totalWidth}px`, height: `${HEADER_H}px` }"
          >
            <div
              class="relative flex h-full items-center px-2"
              :style="{ width: `${graphColWidth}px` }"
            >
              {{ t("git.graph.columns.graph") }}
              <span
                class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
                @pointerdown="startColResize('graph', $event)"
              />
            </div>
            <div
              class="relative flex h-full items-center border-l px-2"
              :style="{ width: `${descColWidth}px` }"
            >
              {{ t("git.graph.columns.description") }}
              <!-- 拖拽按增量调整;双击清除增量恢复完全自适应 -->
              <span
                class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
                @pointerdown="startColResize('desc', $event)"
                @dblclick="colWidths.descDelta = 0"
              />
            </div>
            <div
              class="relative flex h-full items-center border-l px-2"
              :style="{ width: `${colWidths.author}px` }"
            >
              {{ t("git.graph.columns.author") }}
              <span
                class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
                @pointerdown="startColResize('author', $event)"
              />
            </div>
            <div
              class="relative flex h-full items-center border-l px-2"
              :style="{ width: `${colWidths.commit}px` }"
            >
              {{ t("git.graph.columns.commit") }}
              <span
                class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
                @pointerdown="startColResize('commit', $event)"
              />
            </div>
            <div
              class="relative flex h-full items-center border-l px-2"
              :style="{ width: `${colWidths.date}px` }"
            >
              {{ t("git.graph.columns.date") }}
              <span
                class="absolute top-0 right-0 z-10 h-full w-1.5 translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
                @pointerdown="startColResize('date', $event)"
              />
            </div>
          </div>

          <div
            class="relative"
            :style="{ height: `${totalCount * ROW_H}px`, width: `${totalWidth}px` }"
          >
            <!-- 泳道连线与节点(SVG 只覆盖可视窗口;overflow-visible 让穿越窗口的长线完整绘制;
                 图谱列被拖窄时经 clip-path 只做水平裁剪,泳道间距不变;
                 z-10 让图形压在行选中/hover 背景之上,避免高亮盖住节点与连线) -->
            <svg
              class="pointer-events-none absolute left-0 z-10 overflow-visible"
              :style="{
                top: `${svgOffsetY}px`,
                width: `${graphWidth}px`,
                height: `${(endIndex - startIndex + 1) * ROW_H}px`,
                clipPath: graphClipPath,
              }"
            >
              <path
                v-for="(e, i) in visibleEdges"
                :key="i"
                :d="edgePath(e)"
                :stroke="laneColor(e.color)"
                stroke-width="1.5"
                fill="none"
              />
              <template v-for="{ data: n, index } in visibleNodes" :key="n.commit.hash">
                <circle
                  v-if="n.commit.is_head"
                  :cx="nodeX(n.lane)"
                  :cy="nodeYRel(index)"
                  :r="NODE_R + 2.5"
                  fill="none"
                  :stroke="laneColor(n.color)"
                  stroke-width="1.5"
                />
                <circle
                  :cx="nodeX(n.lane)"
                  :cy="nodeYRel(index)"
                  :r="selected?.hash === n.commit.hash ? NODE_R + 1 : NODE_R"
                  :fill="laneColor(n.color)"
                  class="stroke-background"
                  stroke-width="2"
                />
              </template>
            </svg>

            <!-- 提交信息行(虚拟列表,仅渲染可视窗口内的行;单元格宽度与表头列一致) -->
            <div
              v-for="{ data: n, index } in visibleNodes"
              :key="n.commit.hash"
              class="absolute left-0 flex cursor-pointer items-center transition-colors hover:bg-accent/60"
              :class="
                selected?.hash === n.commit.hash
                  ? 'bg-accent'
                  : matchHashes.has(n.commit.hash)
                    ? 'bg-amber-500/15'
                    : ''
              "
              :style="{
                top: `${index * ROW_H}px`,
                height: `${ROW_H}px`,
                width: `${totalWidth}px`,
              }"
              @click="toggleSelect(n.commit)"
            >
              <!-- 图谱列占位:节点与连线由 SVG 覆盖绘制 -->
              <div class="h-full shrink-0" :style="{ width: `${graphColWidth}px` }" />
              <div
                class="flex h-full min-w-0 shrink-0 items-center gap-2 overflow-hidden px-2"
                :style="{ width: `${descColWidth}px` }"
              >
                <Badge
                  v-if="n.commit.is_head"
                  variant="default"
                  class="h-5 shrink-0 px-1.5 text-[10px]"
                >
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
              </div>
              <span
                class="shrink-0 truncate px-2 text-xs text-muted-foreground"
                :style="{ width: `${colWidths.author}px` }"
              >
                {{ n.commit.author }}
              </span>
              <span
                class="shrink-0 truncate px-2 font-mono text-xs text-muted-foreground"
                :style="{ width: `${colWidths.commit}px` }"
              >
                {{ shortHash(n.commit.hash) }}
              </span>
              <span
                class="shrink-0 truncate px-2 text-xs text-muted-foreground"
                :style="{ width: `${colWidths.date}px` }"
              >
                {{ n.commit.date }}
              </span>
            </div>
          </div>

          <p
            v-if="!streamDone"
            class="sticky bottom-0 border-t bg-background/95 px-4 py-2 text-center text-xs text-muted-foreground backdrop-blur"
          >
            {{ t("git.graph.loadingMore", { count: totalCount }) }}
          </p>
        </template>
      </div>

      <!-- 右侧提交详情分栏:选中提交行时展示,拖拽分隔条调宽(把手以边线为中心);
           面板头部可折叠,折叠后保留窄条,点击窄条重新展开 -->
      <template v-if="selected">
        <template v-if="detailOpen">
          <div class="relative w-0 shrink-0">
            <div
              class="absolute inset-y-0 left-0 z-10 w-1.5 -translate-x-1/2 cursor-col-resize transition-colors hover:bg-primary/50"
              @pointerdown="startDetailResize"
            />
          </div>
          <aside class="shrink-0 border-l" :style="{ width: `${effectiveDetailWidth}px` }">
            <CommitDetailPanel
              :commit="selected"
              :project-path="project.path"
              @collapse="detailOpen = false"
            />
          </aside>
        </template>
        <!-- 折叠后的窄条:整条可点击重新展开 -->
        <button
          v-else
          class="flex w-8 shrink-0 items-start justify-center border-l pt-2.5 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          :title="t('git.graph.toggleDetail')"
          @click="detailOpen = true"
        >
          <PanelRightOpen class="h-3.5 w-3.5" />
        </button>
      </template>
    </div>

    <!-- 拉取产生合并冲突时的解决引导(仅当前分支的 pull 可能出现) -->
    <ConflictDialog v-model:open="conflictOpen" :project="project" :conflicts="conflictFiles" />

    <!-- 删除本地分支确认;未合并分支报错后切换为强制删除确认 -->
    <GitBranchDeleteDialog
      v-model:open="deleteOpen"
      :branch="deleteTarget"
      :needs-force="deleteNeedsForce"
      :deleting="deleting"
      @confirm="confirmDeleteBranch"
    />
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
