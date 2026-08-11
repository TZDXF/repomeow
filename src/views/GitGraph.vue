<script setup lang="ts">
import { computed, ref, shallowRef, triggerRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { toast } from "vue-sonner";
import { Channel } from "@tauri-apps/api/core";
import { useVirtualList } from "@vueuse/core";
import {
  ArrowLeft,
  ChevronDown,
  Copy,
  GitBranch,
  Globe,
  ListFilter,
  Loader2,
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

const branches = ref<GitBranches>({ local: [], remote: [] });
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
// 流式批次大小(后端钳制 50..2000)
const BATCH_SIZE = 500;

// --- 增量泳道布局:push 就地追加,批次到达后 triggerRef 触发视图更新 ---
const layouter = createGraphLayouter();
const nodes = shallowRef<GraphNodeLayout[]>(layouter.nodes);
const edges = shallowRef<GraphEdgeLayout[]>([]);
const laneCount = ref(1);
const totalCount = computed(() => nodes.value.length);

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
    <header class="flex items-center gap-2 border-b px-4 py-3">
      <Button variant="ghost" size="sm" @click="router.push(`/projects/${project.id}`)">
        <ArrowLeft class="h-4 w-4" />
        {{ t("git.graph.back") }}
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

    <div class="flex min-h-0 flex-1">
      <!-- 左侧分支/标签列表(SourceTree 风格),点击定位到顶端提交 -->
      <aside v-if="hasSidebar" class="w-52 shrink-0 overflow-y-auto border-r">
        <div class="flex flex-col gap-0.5 px-2 py-2">
          <template v-if="branches.local.length">
            <p
              class="px-1 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase"
            >
              {{ t("git.branch.local") }}
            </p>
            <button
              v-for="b in branches.local"
              :key="b"
              class="flex items-center gap-1.5 rounded-sm px-2 py-1 text-left text-xs transition-colors hover:bg-accent"
              :class="[
                b === currentBranch ? 'font-semibold' : '',
                selectedBranch === b ? 'bg-accent' : '',
              ]"
              @click="selectBranch(b)"
            >
              <GitBranch class="h-3 w-3 shrink-0 text-muted-foreground" />
              <span class="truncate">{{ b }}</span>
              <span
                v-if="b === currentBranch"
                class="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-green-500"
              />
            </button>
          </template>
          <template v-if="branches.remote.length">
            <p
              class="px-1 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase"
            >
              {{ t("git.branch.remote") }}
            </p>
            <button
              v-for="r in branches.remote"
              :key="r"
              class="flex items-center gap-1.5 rounded-sm px-2 py-1 text-left text-xs transition-colors hover:bg-accent"
              :class="selectedBranch === r ? 'bg-accent' : ''"
              @click="selectBranch(r)"
            >
              <Globe class="h-3 w-3 shrink-0 text-muted-foreground" />
              <span class="truncate">{{ r }}</span>
            </button>
          </template>
          <template v-if="tags.length">
            <p
              class="px-1 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase"
            >
              {{ t("git.graph.tags") }}
            </p>
            <button
              v-for="tag in tags"
              :key="tag"
              class="flex items-center gap-1.5 rounded-sm px-2 py-1 text-left text-xs transition-colors hover:bg-accent"
              @click="locateTag(tag)"
            >
              <TagIcon class="h-3 w-3 shrink-0 text-amber-600 dark:text-amber-400" />
              <span class="truncate">{{ tag }}</span>
            </button>
          </template>
        </div>
      </aside>

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
          <div class="relative" :style="{ height: `${totalCount * ROW_H}px` }">
            <!-- 泳道连线与节点(SVG 只覆盖可视窗口;overflow-visible 让穿越窗口的长线完整绘制) -->
            <svg
              class="pointer-events-none absolute left-0 overflow-visible"
              :style="{
                top: `${svgOffsetY}px`,
                width: `${graphWidth}px`,
                height: `${(endIndex - startIndex + 1) * ROW_H}px`,
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
                  r="6.5"
                  fill="none"
                  :stroke="laneColor(n.color)"
                  stroke-width="1.5"
                />
                <circle
                  :cx="nodeX(n.lane)"
                  :cy="nodeYRel(index)"
                  :r="selected?.hash === n.commit.hash ? 5 : 4"
                  :fill="laneColor(n.color)"
                  class="stroke-background"
                  stroke-width="2"
                />
              </template>
            </svg>

            <!-- 提交信息行(虚拟列表,仅渲染可视窗口内的行) -->
            <div
              v-for="{ data: n, index } in visibleNodes"
              :key="n.commit.hash"
              class="absolute right-0 left-0 flex cursor-pointer items-center gap-2 bg-clip-content pr-4 transition-colors hover:bg-accent/60"
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
                paddingLeft: `${graphWidth + 8}px`,
              }"
              @click="toggleSelect(n.commit)"
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
              <span class="shrink-0 text-xs text-muted-foreground">{{ n.commit.author }}</span>
              <span class="shrink-0 font-mono text-xs text-muted-foreground">
                {{ shortHash(n.commit.hash) }}
              </span>
              <span class="shrink-0 text-xs text-muted-foreground">{{ n.commit.date }}</span>
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
