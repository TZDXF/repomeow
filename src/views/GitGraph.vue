<script setup lang="ts">
import { computed, ref, shallowRef, triggerRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { Channel } from "@tauri-apps/api/core";
import { useElementSize, useLocalStorage, useVirtualList } from "@vueuse/core";
import { Loader2, PanelRightOpen } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { createGraphLayouter, type GraphEdgeLayout, type GraphNodeLayout } from "@/lib/git-graph";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import {
  useGitGraphColumnSizing,
  useGitGraphDetailSizing,
} from "@/composables/git/useGitGraphSizing";
import { useGitGraphBranchActions } from "@/composables/git/useGitGraphBranchActions";
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import CommitDetailPanel from "@/components/git/CommitDetailPanel.vue";
import GitBranchDeleteDialog from "@/components/git/GitBranchDeleteDialog.vue";
import GitGraphHeader from "@/components/git/GitGraphHeader.vue";
import GitGraphSidebar from "@/components/git/GitGraphSidebar.vue";
import GitGraphTable from "@/components/git/GitGraphTable.vue";
import GitAnalysisPanel from "@/components/git/analysis/GitAnalysisPanel.vue";
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

// 视图切换:graph = 提交图谱;analysis = 数据分析(全量历史统计面板),选择持久化
const viewMode = useLocalStorage<"graph" | "analysis">("repomeow:graph-view", "graph");
// 分析面板懒激活:首次切入才挂载,之后保持存活,避免来回切换反复全量统计
const analysisActivated = ref(viewMode.value === "analysis");
const analysisPanel = ref<InstanceType<typeof GitAnalysisPanel> | null>(null);

watch(viewMode, (v) => {
  if (v === "analysis") {
    analysisActivated.value = true;
  }
});

/** 头部刷新按当前视图分发:图谱重新流式加载;分析重新统计 */
function onRefresh() {
  if (viewMode.value === "analysis") {
    analysisPanel.value?.reload();
  } else {
    load();
  }
}

const currentBranch = computed(() => project.value?.git?.branch ?? "");

const ROW_H = 32;
const LANE_W = 16;
const GRAPH_PAD = 4;
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

const {
  askDeleteBranch,
  branchOp,
  confirmDeleteBranch,
  conflictFiles,
  conflictOpen,
  deleteNeedsForce,
  deleteOpen,
  deleteTarget,
  deleting,
  pullBranch,
  pushBranch,
} = useGitGraphBranchActions(project, selectedBranch, load);

function toggleSelect(commit: GitGraphCommit) {
  selected.value = selected.value?.hash === commit.hash ? null : commit;
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
    <GitGraphHeader
      v-model:search-query="searchQuery"
      v-model:filter-value="filterValue"
      v-model:view="viewMode"
      :project-name="project.name"
      :search-results="searchResults"
      :filter-label="filterLabel"
      :resolved-filter-branch="resolvedFilterBranch"
      :total-count="totalCount"
      :loading="loading"
      @back="router.push(`/projects/${project.id}`)"
      @locate="locateCommit"
      @refresh="onRefresh"
    />

    <div v-show="viewMode === 'graph'" ref="mainRowEl" class="flex min-h-0 flex-1">
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

        <GitGraphTable
          v-else
          :visible-nodes="visibleNodes"
          :edges="edges"
          :start-index="startIndex"
          :end-index="endIndex"
          :total-count="totalCount"
          :stream-done="streamDone"
          :selected-hash="selected?.hash"
          :match-hashes="matchHashes"
          :graph-width="graphWidth"
          :graph-col-width="graphColWidth"
          :graph-clip-path="graphClipPath"
          :desc-col-width="descColWidth"
          :total-width="totalWidth"
          :col-widths="colWidths"
          @select="toggleSelect"
          @start-col-resize="startColResize"
          @reset-desc-width="colWidths.descDelta = 0"
        />
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

    <!-- 数据分析视图:首次切入才挂载,之后保持存活;替换整个图谱主行(全宽仪表盘) -->
    <GitAnalysisPanel
      v-if="analysisActivated"
      v-show="viewMode === 'analysis'"
      ref="analysisPanel"
      class="min-h-0 flex-1"
      :path="project.path"
    />

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
