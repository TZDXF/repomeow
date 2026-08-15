<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useElementSize, useLocalStorage } from "@vueuse/core";
import {
  ChevronDown,
  ChevronRight,
  ChevronUp,
  Columns2,
  Copy,
  ExternalLink,
  FoldVertical,
  FolderTree,
  GitBranch,
  List,
  Loader2,
  PanelRightClose,
  Rows2,
  Tag as TagIcon,
} from "@lucide/vue";
import { Icon } from "@iconify/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { parseDiff, toSideBySideRows, type DiffFold, type DiffLine } from "@/lib/diff";
import { highlightDiffLines } from "@/lib/diff-highlight";
import { fileIcon, folderIcon } from "@/lib/file-icons";
import { buildFileTree, type FileTreeNode } from "@/lib/file-tree";
import { openPathWith, sortOpenWithOptions } from "@/lib/open-with";
import { baseName } from "@/lib/path";
import { cmd } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settings";
import type { GitCommitFile, GitCommitFileDiff, GitGraphCommit } from "@/types";

const props = defineProps<{
  commit: GitGraphCommit;
  /** 项目根目录绝对路径(拼接文件相对路径后用于 IDE 打开) */
  projectPath: string;
}>();

const emit = defineEmits<{
  /** 折叠为右侧窄条(窄条由父组件渲染,点击重新展开) */
  collapse: [];
}>();

const { t } = useI18n();
const settings = useSettingsStore();

// --- 文件列表 ---
const files = ref<GitCommitFile[]>([]);
const filesLoading = ref(false);
const filesError = ref("");
/** 平铺 / 树形切换(持久化) */
const treeMode = useLocalStorage("repomeow:commit-files-tree", false);
const collapsedFolders = ref(new Set<string>());

// --- 单文件 diff ---
const selectedPath = ref<string | null>(null);
const diff = ref<GitCommitFileDiff | null>(null);
const diffLoading = ref(false);
const diffError = ref("");

/** 合并提交(多父)diff-tree 无输出,展示提示而非空列表 */
const isMerge = computed(() => props.commit.parents.length > 1);

async function loadFiles() {
  filesLoading.value = true;
  filesError.value = "";
  files.value = [];
  selectedPath.value = null;
  diff.value = null;
  diffLines.value = [];
  lineHtml.value = new Map();
  diffError.value = "";
  try {
    files.value = await cmd<GitCommitFile[]>("git_commit_files", {
      path: props.projectPath,
      hash: props.commit.hash,
    });
    // 默认选中第一个文件,直接展示 diff
    if (files.value.length) {
      void selectFile(files.value[0]);
    }
  } catch (e) {
    filesError.value = String(e);
  } finally {
    filesLoading.value = false;
  }
}

watch(() => props.commit.hash, loadFiles, { immediate: true });

async function selectFile(file: GitCommitFile) {
  if (selectedPath.value === file.path && diff.value) {
    return;
  }
  selectedPath.value = file.path;
  // 切文件不清空旧 diff、不转圈:旧内容保留到新内容就绪后同帧替换;
  // 仅首次加载(没有任何旧内容)时 diffLoading 才为 true,给空区域一个转圈
  diffLoading.value = !diff.value;
  diffError.value = "";
  try {
    const result = await cmd<GitCommitFileDiff>("git_commit_file_diff", {
      path: props.projectPath,
      hash: props.commit.hash,
      filePath: file.path,
      oldPath: file.old_path,
    });
    // 快速连点 A→B:旧响应解析后可能晚于新请求的 selectedPath,
    // 不做 stale 校验会把 A 的 diff 写到 B 的标题下,先于 B 的真实结果
    if (selectedPath.value !== file.path) return;
    // 先解析再着色,完成后 diff / 行模型 / 高亮同一帧落地:避免纯文本先闪现再上色。
    // 行模型直接复用这份 parseDiff 结果,保证 lineHtml 的键与模板渲染的是同一批对象引用;
    // 折叠/滚动位置属旧 diff,也等这一帧一起重置
    const lines = parseDiff(result.diff);
    const htmlMap = (await highlightDiffLines(lines, file.path)) ?? new Map<DiffLine, string>();
    if (selectedPath.value !== file.path) return;
    lineHtml.value = htmlMap;
    diffLines.value = lines;
    diff.value = result;
    expandedFolds.value = new Set();
    currentScrollTop.value = 0;
  } catch (e) {
    if (selectedPath.value !== file.path) return;
    diffError.value = String(e);
  } finally {
    if (selectedPath.value === file.path) diffLoading.value = false;
  }
}

// --- 树形展示:按目录层级聚合,折叠状态记忆 ---
const fileTree = computed(() => buildFileTree(files.value));

interface FileRow {
  node: FileTreeNode;
  depth: number;
}

/** 拍平可见树行(跳过折叠目录的子级) */
const treeRows = computed(() => {
  const out: FileRow[] = [];
  const walk = (nodes: FileTreeNode[], depth: number) => {
    for (const node of nodes) {
      out.push({ node, depth });
      if (node.children.length && !collapsedFolders.value.has(node.fullPath)) {
        walk(node.children, depth + 1);
      }
    }
  };
  walk(fileTree.value, 0);
  return out;
});

function toggleFolder(fullPath: string) {
  const next = new Set(collapsedFolders.value);
  if (next.has(fullPath)) {
    next.delete(fullPath);
  } else {
    next.add(fullPath);
  }
  collapsedFolders.value = next;
}

// --- diff 解析:lib/diff.ts 的 parseDiff(与提交对话框变更预览共用);baseName 走 @/lib/path ---
// 后端 context_lines 已拉满,diff 含完整文件内容;过长的未更改区间折叠为可点击展开的占位行(IDEA 风格)。
// 保留 hunk 头:对超大文件(>10 万行)后端 libgit2 在 @@ 上报畸形头(2- 之类),
// 此时 truncated 为 true,hunk 头是定位"被截断的变更段"的唯一线索;truncated=false 时仍是 @@ -1,N +1,M @@,
// 展示一行灰色分隔对正常文件无明显影响。
// diffLines 不是 computed:由 selectFile 在着色完成后与 diff / lineHtml 同帧写入(见 selectFile 注释)
const diffLines = shallowRef<DiffLine[]>([]);

/** 逐行着色结果:DiffLine → 行内 HTML;键就是 diffLines 里的对象引用(fold/sideRows 复用同一批) */
const lineHtml = shallowRef(new Map<DiffLine, string>());

/** 超过该行数的连续未更改区间才折叠 */
const FOLD_MIN = 12;
/** 折叠区间两端各保留的上下文行数 */
const FOLD_EDGE = 3;
/** 已手动展开的折叠区(换文件时重置) */
const expandedFolds = ref(new Set<string>());

function foldCtxRuns(lines: DiffLine[]): (DiffLine | DiffFold)[] {
  const out: (DiffLine | DiffFold)[] = [];
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind !== "ctx") {
      out.push(lines[i]);
      i++;
      continue;
    }
    let j = i;
    while (j < lines.length && lines[j].kind === "ctx") j++;
    const len = j - i;
    const key = `${i}:${len}`;
    if (len > FOLD_MIN && !expandedFolds.value.has(key)) {
      out.push(...lines.slice(i, i + FOLD_EDGE));
      out.push({ kind: "fold", count: len - FOLD_EDGE * 2, key });
      out.push(...lines.slice(j - FOLD_EDGE, j));
    } else {
      out.push(...lines.slice(i, j));
    }
    i = j;
  }
  return out;
}

function expandFold(key: string) {
  if (!key) return;
  const next = new Set(expandedFolds.value);
  next.add(key);
  expandedFolds.value = next;
}

/** 模板取 fold 字段的访问器(避免依赖模板内联合类型收窄) */
function foldKeyOf(line: DiffLine | DiffFold) {
  return line.kind === "fold" ? line.key : "";
}
function foldCountOf(line: DiffLine | DiffFold) {
  return line.kind === "fold" ? line.count : 0;
}

const displayLines = computed(() => foldCtxRuns(diffLines.value));

/** 模板取行高亮 HTML(未着色时为空串,回退纯文本渲染) */
function hlOf(line: DiffLine | null | undefined) {
  return (line && lineHtml.value.get(line)) || "";
}

/** 并排查看(持久化):旧版本在左、新版本在右 */
const splitDiff = useLocalStorage("repomeow:commit-diff-split", false);
const sideRows = computed(() => toSideBySideRows(displayLines.value));

/** 并排视图去掉行首 diff 标记(+ / - / 空格) */
function sideText(line: DiffLine | null) {
  return line ? line.text.slice(1) : "";
}

// --- 并排视图行模型:一侧空白的连续行压缩为一条示意线(gap) ---
// gap 高度 = 行数 × 1.25rem(与模板 leading-5 行高一致),两窗格总高度不变,滚动同步不受影响
interface PaneRow {
  kind: "hunk" | "meta" | "line" | "gap" | "fold";
  text: string;
  line: DiffLine | null;
  /** gap: 压缩的空白行数;fold: 折叠的行数 */
  count: number;
  tint: "red" | "green";
  /** fold 行的展开状态键(其余行为空串) */
  foldKey: string;
}

function buildPaneRows(side: "left" | "right"): PaneRow[] {
  const out: PaneRow[] = [];
  for (const row of sideRows.value) {
    if (row.kind === "fold") {
      out.push({
        kind: "fold",
        text: "",
        line: null,
        count: row.fold?.count ?? 0,
        tint: "red",
        foldKey: row.fold?.key ?? "",
      });
      continue;
    }
    if (row.kind !== "line") {
      out.push({ kind: row.kind, text: row.text, line: null, count: 0, tint: "red", foldKey: "" });
      continue;
    }
    const line = side === "left" ? row.left : row.right;
    if (line) {
      out.push({ kind: "line", text: "", line, count: 0, tint: "red", foldKey: "" });
      continue;
    }
    // 本侧空白:染色跟随对侧内容(del 红 / add 绿),连续空白合并为一条示意线
    const other = side === "left" ? row.right : row.left;
    const tint = other?.kind === "del" ? "red" : "green";
    const last = out[out.length - 1];
    if (last?.kind === "gap" && last.tint === tint) {
      last.count++;
    } else {
      out.push({ kind: "gap", text: "", line: null, count: 1, tint, foldKey: "" });
    }
  }
  return out;
}

const leftRows = computed(() => buildPaneRows("left"));
const rightRows = computed(() => buildPaneRows("right"));

// --- 并排视图滚动:内容窗格 + 中间行号栏三栏镜像同步;行号栏无横向滚动,横向只在两内容窗格间同步 ---
const leftPaneEl = ref<HTMLElement | null>(null);
const rightPaneEl = ref<HTMLElement | null>(null);
const gutterEl = ref<HTMLElement | null>(null);
// per-instance 状态(用 ref 避免被 <script setup> 模块作用域共享,多实例并发时互不干扰)
const paneSyncing = ref(false);
let paneSyncFrame = 0;

function visualScrollLeft(el: HTMLElement) {
  return el === leftPaneEl.value ? el.scrollWidth - el.clientWidth - el.scrollLeft : el.scrollLeft;
}

function applyVisualScrollLeft(el: HTMLElement, offset: number) {
  el.scrollLeft = el === leftPaneEl.value ? el.scrollWidth - el.clientWidth - offset : offset;
}

function syncPaneScroll(source: "left" | "right" | "gutter") {
  if (paneSyncing.value) return;
  const from = { left: leftPaneEl, right: rightPaneEl, gutter: gutterEl }[source].value;
  if (!from) return;
  currentScrollTop.value = from.scrollTop;
  paneSyncing.value = true;
  const hOffset = from === gutterEl.value ? 0 : visualScrollLeft(from);
  for (const el of [leftPaneEl.value, rightPaneEl.value, gutterEl.value]) {
    if (!el || el === from) continue;
    el.scrollTop = from.scrollTop;
    if (el !== gutterEl.value && from !== gutterEl.value) {
      applyVisualScrollLeft(el, hOffset);
    }
  }
  paneSyncFrame = requestAnimationFrame(() => {
    paneSyncing.value = false;
  });
}

// 左窗格 -scale-x-100 翻转后,浏览器原生横向滚轮仍按翻转前的布局坐标增减 scrollLeft,
// 方向与可视滚动条相反;拦截横向分量取反后手动滚动,纵向分量交给默认行为
function onLeftPaneWheel(e: WheelEvent) {
  const delta = e.deltaX !== 0 ? e.deltaX : e.shiftKey ? e.deltaY : 0;
  const el = leftPaneEl.value;
  if (!delta || !el) return;
  e.preventDefault();
  const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? el.clientWidth : 1;
  el.scrollLeft -= delta * unit;
}

/** 当前选中文件(已删除的文件不提供 IDE 打开:工作区已不存在) */
const selectedFile = computed(() => files.value.find((f) => f.path === selectedPath.value) ?? null);
const canOpenInIde = computed(() => selectedFile.value?.status !== "D");

/** 并排是否适用:新增/删除文件一侧必然全空,强制逐行视图并隐藏切换按钮 */
const splitApplicable = computed(
  () => selectedFile.value?.status !== "A" && selectedFile.value?.status !== "D",
);
const splitActive = computed(() => splitDiff.value && splitApplicable.value);

// 打开并排视图 / 新 diff 内容落地后,把左窗格 scrollLeft 推到最大(翻转栏的可视起点),
// 让两侧都从行首看起;赋值会触发 scroll 事件,经 syncPaneScroll 顺带把右窗格归零。
// 依赖 diff 而非 selectedPath / diffLoading:切文件不再清空旧内容,只有新内容就绪这一帧才需要复位横向滚动
watch([splitActive, diff], async ([active]) => {
  if (!active) return;
  await nextTick();
  const lp = leftPaneEl.value;
  if (lp) lp.scrollLeft = lp.scrollWidth - lp.clientWidth;
});

// --- 在 IDE 打开(默认编辑器) ---
async function openFile(file: GitCommitFile) {
  const option = sortOpenWithOptions(settings.openWithOrder, settings.customOpenWith).find(
    (candidate) => candidate.id === settings.defaultOpenWith,
  );
  if (!option) return;
  try {
    await openPathWith(option, `${props.projectPath}/${file.path}`);
  } catch (e) {
    toast.error(String(e));
  }
}

// --- 提交信息辅助(与图谱页底部旧面板一致) ---
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

// --- 状态徽标配色 ---
function statusClass(status: string) {
  switch (status) {
    case "A":
      return "text-green-600 dark:text-green-400";
    case "D":
      return "text-red-600 dark:text-red-400";
    case "R":
      return "text-blue-600 dark:text-blue-400";
    case "T":
      return "text-purple-600 dark:text-purple-400";
    default:
      return "text-amber-600 dark:text-amber-400";
  }
}

// --- 布局:vertical = 列表在上 diff 在下;horizontal = 列表与 diff 左右分列(diff 单独一列) ---
/** 布局切换(持久化) */
const layout = useLocalStorage<"vertical" | "horizontal">(
  "repomeow:commit-detail-layout",
  "vertical",
);

// 布局切换(上下 ↔ 左右)后:旧 scrollTop 可能不再对应新容器的内容位置,
// 直接归零让 chevron 上/下按钮按新视图的"顶端"重新判定 hasPrev/Next;
/// syncHbarPad 已经在 watch 列表里,这里只补 currentScrollTop
watch(layout, () => {
  currentScrollTop.value = 0;
});

// --- 文件列表 / diff 区分隔拖拽:上下布局调列表高度,左右布局调列表宽度 ---
const listHeight = useLocalStorage("repomeow:commit-detail-list-h", 240);
const listWidth = useLocalStorage("repomeow:commit-detail-list-w", 280);
const LIST_MIN_H = 120;
const LIST_MAX_H = 600;
const LIST_MIN_W = 180;
/** 左右布局下 diff 列的最小宽度:listWidth 持久化值可能超过当前面板宽度(面板被拖窄过),
 * 列表宽度按容器实测宽度自适应 clamp,避免把 diff 挤成负宽、列表溢出容器 */
const DIFF_MIN_W = 120;
const rootEl = ref<HTMLElement | null>(null);
const { width: panelWidth } = useElementSize(rootEl);

/** 列表宽度实际上限:面板宽减去 diff 列最小宽度,随面板宽度动态伸缩(不设固定上限) */
function listWidthCap() {
  if (layout.value !== "horizontal" || !panelWidth.value) return LIST_MIN_W;
  return Math.max(LIST_MIN_W, Math.floor(panelWidth.value) - DIFF_MIN_W);
}

// --- 三栏横向滚动条补偿:横向滚动条吃掉出现它的栏的可视高度,各栏最大 scrollTop 因此不一致
// (滚到底时行号与代码错位);量出各栏横向滚动条高度,给没有的栏补等量底部内边距,拉平可滚范围 ---
const hbarPad = ref({ left: 0, right: 0, gutter: 0 });

async function syncHbarPad() {
  await nextTick();
  const els = [leftPaneEl.value, rightPaneEl.value, gutterEl.value];
  const hb = els.map((el) => (el ? el.offsetHeight - el.clientHeight : 0));
  const max = Math.max(...hb);
  hbarPad.value = { left: max - hb[0], right: max - hb[1], gutter: max - hb[2] };
}

// 内容 / 布局 / 面板与列表宽度变化都可能改变横向滚动条的出现与否
watch([displayLines, splitActive, panelWidth, listWidth, layout], () => void syncHbarPad(), {
  flush: "post",
});

// --- 差异导航:上/下一个差异按行索引 × 行高定位;行高与模板 h-5 / leading-5(1.25rem)一致 ---
const unifiedEl = ref<HTMLElement | null>(null);
const currentScrollTop = ref(0);

function rowHeightPx() {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) * 1.25 || 20;
}

/** 取各连续变更块的首行下标(flags 中 false→true 的跳变位置) */
function blockStarts(flags: boolean[]) {
  const out: number[] = [];
  flags.forEach((f, i) => {
    if (f && !flags[i - 1]) {
      out.push(i);
    }
  });
  return out;
}

/** 差异块首行在滚动内容中的行索引(连续增删算一个差异):并排按 sideRows,逐行按 displayLines */
const changeRowIdx = computed(() => {
  if (splitActive.value) {
    return blockStarts(
      sideRows.value.map(
        (row) => row.kind === "line" && (row.left?.kind === "del" || row.right?.kind === "add"),
      ),
    );
  }
  return blockStarts(displayLines.value.map((line) => line.kind === "add" || line.kind === "del"));
});

/** 差异行顶部对应的 scrollTop(内容容器有 py-1 上内边距 = 行高的 1/5) */
function changeOffsets() {
  const h = rowHeightPx();
  return changeRowIdx.value.map((i) => h / 5 + i * h);
}

const hasPrevChange = computed(() => changeOffsets().some((o) => o < currentScrollTop.value - 2));
const hasNextChange = computed(() => changeOffsets().some((o) => o > currentScrollTop.value + 2));

function scrollToChange(dir: 1 | -1) {
  const offsets = changeOffsets();
  const cur = currentScrollTop.value;
  const target =
    dir === 1 ? offsets.find((o) => o > cur + 2) : [...offsets].reverse().find((o) => o < cur - 2);
  if (target == null) return;
  currentScrollTop.value = target;
  // 直接给各栏赋值,不等 scroll 事件传播
  for (const col of [leftPaneEl.value, rightPaneEl.value, gutterEl.value, unifiedEl.value]) {
    if (col) col.scrollTop = target;
  }
}

function onUnifiedScroll() {
  currentScrollTop.value = unifiedEl.value?.scrollTop ?? 0;
}

/** 收起全部已展开的未更改片段 */
const hasExpandedFolds = computed(() => expandedFolds.value.size > 0);
function collapseAllFolds() {
  expandedFolds.value = new Set();
}

/** 渲染用列表宽度(不改持久化值,面板变宽后用户原设定自然恢复) */
const effectiveListWidth = computed(() => Math.min(listWidth.value, listWidthCap()));

// 分隔条拖拽中的全局监听器,unmount 时统一摘掉,避免组件被卸载而监听器还活着
let listResizeCleanups: (() => void)[] = [];

function startListResize(e: PointerEvent) {
  e.preventDefault();
  const horizontal = layout.value === "horizontal";
  const startPos = horizontal ? e.clientX : e.clientY;
  const startSize = horizontal ? listWidth.value : listHeight.value;
  const onMove = (ev: PointerEvent) => {
    const size = Math.round(startSize + (horizontal ? ev.clientX : ev.clientY) - startPos);
    if (horizontal) {
      listWidth.value = Math.min(listWidthCap(), Math.max(LIST_MIN_W, size));
    } else {
      listHeight.value = Math.min(LIST_MAX_H, Math.max(LIST_MIN_H, size));
    }
  };
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    listResizeCleanups = listResizeCleanups.filter((fn) => fn !== cleanup);
  };
  const cleanup = onUp;
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  listResizeCleanups.push(cleanup);
}

onBeforeUnmount(() => {
  // 拖拽中组件被卸载:残留 pointermove/pointerup 监听器仍会引用已卸载组件的状态,
  // 显式移除避免泄漏;rAF 同步锁也一并 cancel,防止回调在卸载后改 ref
  for (const fn of listResizeCleanups) fn();
  listResizeCleanups = [];
  if (paneSyncFrame) cancelAnimationFrame(paneSyncFrame);
});
</script>

<template>
  <div ref="rootEl" class="flex h-full flex-col">
    <!-- 提交信息 -->
    <div class="shrink-0 border-b px-3 py-2.5">
      <div class="flex items-start justify-between gap-2">
        <p class="max-h-15 min-w-0 overflow-y-auto text-sm font-medium break-all">
          {{ commit.subject }}
        </p>
        <Button
          variant="ghost"
          size="sm"
          class="h-6 w-6 shrink-0 p-0"
          :title="t('git.graph.toggleDetail')"
          @click="emit('collapse')"
        >
          <PanelRightClose class="h-4 w-4" />
        </Button>
      </div>
      <div class="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
        <span class="flex items-center gap-1">
          {{ t("git.graph.detail.hash") }}
          <code class="font-mono text-foreground">{{ shortHash(commit.hash) }}</code>
          <button
            class="text-muted-foreground transition-colors hover:text-foreground"
            :title="t('git.graph.copyHash')"
            @click="copyHash(commit.hash)"
          >
            <Copy class="h-3 w-3" />
          </button>
        </span>
        <span>{{ t("git.graph.detail.author") }} {{ commit.author }}</span>
        <span>{{ t("git.graph.detail.date") }} {{ commit.date }}</span>
        <span v-if="commit.parents.length" class="font-mono">
          {{ t("git.graph.detail.parents") }}
          {{ commit.parents.map(shortHash).join(", ") }}
        </span>
      </div>
      <div v-if="commit.refs.length" class="mt-1.5 flex flex-wrap gap-1">
        <Badge
          v-for="r in commit.refs"
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

    <!-- 主体:上下布局(列表在上 diff 在下)/ 左右布局(diff 单独一列) -->
    <div class="flex min-h-0 flex-1" :class="layout === 'horizontal' ? 'flex-row' : 'flex-col'">
      <!-- 文件列表:平铺 / 树形切换 -->
      <div
        class="flex min-h-0 min-w-0 shrink-0 flex-col"
        :style="
          layout === 'horizontal'
            ? { width: `${effectiveListWidth}px` }
            : { height: `${listHeight}px` }
        "
      >
        <div class="flex shrink-0 items-center justify-between border-b px-3 py-1.5">
          <span class="text-xs font-medium text-muted-foreground">
            {{ t("git.graph.detail.filesCount", { count: files.length }) }}
          </span>
          <div class="flex items-center">
            <button
              class="rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              :title="
                t(
                  layout === 'horizontal'
                    ? 'git.graph.detail.layoutStack'
                    : 'git.graph.detail.layoutSplit',
                )
              "
              @click="layout = layout === 'horizontal' ? 'vertical' : 'horizontal'"
            >
              <Rows2 v-if="layout === 'horizontal'" class="h-3.5 w-3.5" />
              <Columns2 v-else class="h-3.5 w-3.5" />
            </button>
            <button
              class="rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              :title="t(treeMode ? 'git.graph.detail.showFlat' : 'git.graph.detail.showTree')"
              @click="treeMode = !treeMode"
            >
              <List v-if="treeMode" class="h-3.5 w-3.5" />
              <FolderTree v-else class="h-3.5 w-3.5" />
            </button>
          </div>
        </div>

        <div class="min-h-0 flex-1 overflow-auto py-1">
          <div v-if="filesLoading" class="flex h-full items-center justify-center">
            <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
          <p v-else-if="filesError" class="px-3 py-2 text-xs text-destructive">
            {{ t("git.graph.detail.filesLoadFailed") }}:{{ filesError }}
          </p>
          <p v-else-if="!files.length" class="px-3 py-2 text-xs text-muted-foreground">
            {{ t(isMerge ? "git.graph.detail.mergeCommit" : "git.graph.detail.emptyFiles") }}
          </p>

          <!-- 平铺 -->
          <template v-else-if="!treeMode">
            <div
              v-for="file in files"
              :key="file.path"
              class="flex w-full cursor-pointer items-center gap-1.5 px-3 py-1 text-xs transition-colors hover:bg-accent/60"
              :class="selectedPath === file.path ? 'bg-accent' : ''"
              @click="selectFile(file)"
            >
              <span class="w-3 shrink-0 font-mono font-semibold" :class="statusClass(file.status)">
                {{ file.status }}
              </span>
              <Icon :icon="fileIcon(baseName(file.path))" class="h-3.5 w-3.5 shrink-0" />
              <span
                class="min-w-0 flex-1 truncate font-mono"
                :title="file.old_path ? `${file.old_path} → ${file.path}` : file.path"
              >
                <template v-if="file.old_path">{{ baseName(file.old_path) }} → </template
                >{{ baseName(file.path) }}
              </span>
              <span v-if="file.additions" class="shrink-0 text-green-600 dark:text-green-400">
                +{{ file.additions }}
              </span>
              <span v-if="file.deletions" class="shrink-0 text-red-600 dark:text-red-400">
                -{{ file.deletions }}
              </span>
            </div>
          </template>

          <!-- 树形 -->
          <template v-else>
            <div
              v-for="row in treeRows"
              :key="row.node.fullPath"
              class="flex w-full items-center gap-1.5 py-1 pr-3 text-xs transition-colors"
              :class="[
                row.node.file ? 'cursor-pointer hover:bg-accent/60' : '',
                row.node.file && selectedPath === row.node.file.path ? 'bg-accent' : '',
              ]"
              :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
              @click="row.node.file ? selectFile(row.node.file) : toggleFolder(row.node.fullPath)"
            >
              <span class="w-3 shrink-0 text-muted-foreground">
                <ChevronRight
                  v-if="row.node.children.length"
                  class="h-3 w-3 transition-transform"
                  :class="collapsedFolders.has(row.node.fullPath) ? '' : 'rotate-90'"
                />
              </span>
              <Icon
                v-if="!row.node.file"
                :icon="folderIcon(row.node.name, !collapsedFolders.has(row.node.fullPath))"
                class="h-3.5 w-3.5 shrink-0"
              />
              <template v-else>
                <span
                  class="w-3 shrink-0 font-mono font-semibold"
                  :class="statusClass(row.node.file.status)"
                >
                  {{ row.node.file.status }}
                </span>
                <Icon :icon="fileIcon(row.node.name)" class="h-3.5 w-3.5 shrink-0" />
              </template>
              <span class="min-w-0 flex-1 truncate font-mono" :title="row.node.fullPath">
                {{ row.node.name }}
              </span>
              <template v-if="row.node.file">
                <span
                  v-if="row.node.file.additions"
                  class="shrink-0 text-green-600 dark:text-green-400"
                >
                  +{{ row.node.file.additions }}
                </span>
                <span
                  v-if="row.node.file.deletions"
                  class="shrink-0 text-red-600 dark:text-red-400"
                >
                  -{{ row.node.file.deletions }}
                </span>
              </template>
            </div>
          </template>
        </div>
      </div>

      <!-- 列表 / diff 分隔拖拽条:上下布局横条调高,左右布局竖条调宽 -->
      <div
        class="shrink-0 transition-colors hover:bg-primary/50"
        :class="layout === 'horizontal' ? 'w-1.5 cursor-col-resize' : 'h-1.5 cursor-row-resize'"
        @pointerdown="startListResize"
      />

      <!-- diff 区:自实现逐行渲染(行号 + 增删底色);commit-diff 类供 shiki 双主题变量按 .dark 切换 -->
      <div class="commit-diff flex min-h-0 min-w-0 flex-1 flex-col">
        <div class="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
          <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="selectedFile?.path">
            {{ selectedFile?.path ?? "" }}
          </span>
          <Badge v-if="diff?.truncated" variant="outline" class="h-5 shrink-0 px-1.5 text-[10px]">
            {{ t("git.graph.detail.diffTruncated") }}
          </Badge>
          <template v-if="diff">
            <button
              class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
              :disabled="!hasPrevChange"
              :title="t('git.graph.detail.diffPrevChange')"
              @click="scrollToChange(-1)"
            >
              <ChevronUp class="h-3.5 w-3.5" />
            </button>
            <button
              class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
              :disabled="!hasNextChange"
              :title="t('git.graph.detail.diffNextChange')"
              @click="scrollToChange(1)"
            >
              <ChevronDown class="h-3.5 w-3.5" />
            </button>
            <button
              class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors not-disabled:hover:bg-accent not-disabled:hover:text-foreground disabled:opacity-40"
              :disabled="!hasExpandedFolds"
              :title="t('git.graph.detail.diffCollapseFolds')"
              @click="collapseAllFolds"
            >
              <FoldVertical class="h-3.5 w-3.5" />
            </button>
          </template>
          <button
            v-if="diff && splitApplicable"
            class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            :title="t(splitActive ? 'git.graph.detail.diffUnified' : 'git.graph.detail.diffSplit')"
            @click="splitDiff = !splitDiff"
          >
            <Rows2 v-if="splitDiff" class="h-3.5 w-3.5" />
            <Columns2 v-else class="h-3.5 w-3.5" />
          </button>
          <button
            v-if="selectedFile && canOpenInIde"
            class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            :title="t('git.graph.detail.openInIde')"
            @click="openFile(selectedFile)"
          >
            <ExternalLink class="h-3.5 w-3.5" />
          </button>
        </div>

        <!-- 并排(split)视图:左内容 | 中间行号栏 | 右内容,三栏纵向滚动经 syncPaneScroll 镜像同步 -->
        <div
          v-if="splitActive && !diffLoading && !diffError && selectedPath"
          class="flex min-h-0 flex-1"
        >
          <!-- 左窗格 -scale-x-100 双翻转:容器翻转把纵向滚动条移到左边,内容层再翻转回正;
               代价是 scrollLeft 镜像化(0=内容末尾、最大值=行首),横向同步经 visualScrollLeft/applyVisualScrollLeft 换算 -->
          <div
            ref="leftPaneEl"
            class="min-w-0 flex-1 -scale-x-100 overflow-auto"
            @scroll="syncPaneScroll('left')"
            @wheel="onLeftPaneWheel"
          >
            <div
              class="min-w-max -scale-x-100 py-1 font-mono text-xs leading-5"
              :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.left}px)` }"
            >
              <template v-for="(row, i) in leftRows" :key="i">
                <div
                  v-if="row.kind === 'hunk'"
                  class="bg-muted/60 px-3 whitespace-pre text-muted-foreground select-none"
                >
                  {{ row.text }}
                </div>
                <div
                  v-else-if="row.kind === 'meta'"
                  class="px-3 whitespace-pre text-muted-foreground select-none"
                >
                  {{ row.text }}
                </div>
                <button
                  v-else-if="row.kind === 'fold'"
                  class="block w-full bg-muted/40 select-none hover:bg-accent"
                  :title="t('git.graph.detail.diffExpand', { count: row.count })"
                  @click="expandFold(row.foldKey)"
                >
                  <div class="diff-fold-wave ml-3 h-5" />
                </button>
                <div
                  v-else-if="row.kind === 'gap'"
                  class="flex items-center px-2 select-none"
                  :style="{ height: `${row.count * 1.25}rem` }"
                  :title="t('git.graph.detail.diffGap', { count: row.count })"
                >
                  <div
                    class="h-px flex-1"
                    :class="row.tint === 'red' ? 'bg-red-500/40' : 'bg-green-500/40'"
                  />
                </div>
                <!-- 行高固定 h-5:去掉行首标记后空行内容为空,行盒会塌缩成 0 高导致三栏错位 -->
                <div
                  v-else
                  class="h-5 pl-2"
                  :class="row.line?.kind === 'del' ? 'bg-red-500/10' : ''"
                >
                  <span
                    v-if="hlOf(row.line)"
                    class="diff-hl whitespace-pre"
                    v-html="hlOf(row.line)"
                  />
                  <span v-else class="whitespace-pre">{{ sideText(row.line) }}</span>
                </div>
              </template>
            </div>
          </div>

          <!-- 中间行号栏:左右行号并排放置,不参与横向滚动(滚动条隐藏但可滚),纵向经 syncPaneScroll 同步;
               增删行行号格着色,本侧空白处淡色过渡,与内容区底色衔接 -->
          <div
            ref="gutterEl"
            class="shrink-0 overflow-auto border-x border-border/60 font-mono text-xs leading-5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
            @scroll="syncPaneScroll('gutter')"
          >
            <div class="py-1" :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.gutter}px)` }">
              <template v-for="(row, i) in sideRows" :key="i">
                <div v-if="row.kind === 'hunk'" class="h-5 bg-muted/60 select-none" />
                <div v-else-if="row.kind === 'meta'" class="h-5 select-none" />
                <!-- 波浪线贯穿行号栏,与两侧窗格的折叠行连成一线;同样可点击展开 -->
                <button
                  v-else-if="row.kind === 'fold'"
                  class="block h-5 w-full bg-muted/40 select-none hover:bg-accent"
                  :title="t('git.graph.detail.diffExpand', { count: row.fold?.count ?? 0 })"
                  @click="expandFold(row.fold?.key ?? '')"
                >
                  <div class="diff-fold-wave h-full" />
                </button>
                <div v-else class="flex h-5">
                  <span
                    class="h-full w-10 pr-1 text-right text-muted-foreground/50 select-none"
                    :class="
                      row.left?.kind === 'del'
                        ? 'bg-red-500/10'
                        : !row.left && row.right?.kind === 'add'
                          ? 'bg-green-500/5'
                          : ''
                    "
                    >{{ row.left?.oldLine ?? "" }}</span
                  ><span
                    class="h-full w-10 pl-1 text-right text-muted-foreground/50 select-none"
                    :class="
                      row.right?.kind === 'add'
                        ? 'bg-green-500/10'
                        : !row.right && row.left?.kind === 'del'
                          ? 'bg-red-500/5'
                          : ''
                    "
                    >{{ row.right?.newLine ?? "" }}</span
                  >
                </div>
              </template>
            </div>
          </div>
          <div
            ref="rightPaneEl"
            class="min-w-0 flex-1 overflow-auto"
            @scroll="syncPaneScroll('right')"
          >
            <div
              class="min-w-max py-1 font-mono text-xs leading-5"
              :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.right}px)` }"
            >
              <template v-for="(row, i) in rightRows" :key="i">
                <div
                  v-if="row.kind === 'hunk'"
                  class="bg-muted/60 px-3 whitespace-pre text-muted-foreground select-none"
                >
                  {{ row.text }}
                </div>
                <div
                  v-else-if="row.kind === 'meta'"
                  class="px-3 whitespace-pre text-muted-foreground select-none"
                >
                  {{ row.text }}
                </div>
                <button
                  v-else-if="row.kind === 'fold'"
                  class="block w-full bg-muted/40 select-none hover:bg-accent"
                  :title="t('git.graph.detail.diffExpand', { count: row.count })"
                  @click="expandFold(row.foldKey)"
                >
                  <div class="diff-fold-wave mr-3 h-5" />
                </button>
                <div
                  v-else-if="row.kind === 'gap'"
                  class="flex items-center px-2 select-none"
                  :style="{ height: `${row.count * 1.25}rem` }"
                  :title="t('git.graph.detail.diffGap', { count: row.count })"
                >
                  <div
                    class="h-px flex-1"
                    :class="row.tint === 'red' ? 'bg-red-500/40' : 'bg-green-500/40'"
                  />
                </div>
                <!-- 行高固定 h-5:去掉行首标记后空行内容为空,行盒会塌缩成 0 高导致三栏错位 -->
                <div
                  v-else
                  class="h-5 pl-2"
                  :class="row.line?.kind === 'add' ? 'bg-green-500/10' : ''"
                >
                  <span
                    v-if="hlOf(row.line)"
                    class="diff-hl whitespace-pre"
                    v-html="hlOf(row.line)"
                  />
                  <span v-else class="whitespace-pre">{{ sideText(row.line) }}</span>
                </div>
              </template>
            </div>
          </div>
        </div>

        <div ref="unifiedEl" v-else class="min-h-0 flex-1 overflow-auto" @scroll="onUnifiedScroll">
          <div v-if="diffLoading" class="flex h-full items-center justify-center">
            <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
          <p v-else-if="diffError" class="px-3 py-2 text-xs text-destructive">
            {{ t("git.graph.detail.diffLoadFailed") }}:{{ diffError }}
          </p>
          <p
            v-else-if="!selectedPath"
            class="flex h-full items-center justify-center text-xs text-muted-foreground"
          >
            {{ t("git.graph.detail.selectFile") }}
          </p>

          <!-- 逐行(unified)视图 -->
          <div v-else class="min-w-max py-1 font-mono text-xs leading-5">
            <template v-for="(line, i) in displayLines" :key="i">
              <div
                v-if="line.kind === 'hunk'"
                class="bg-muted/60 px-3 text-muted-foreground select-none"
              >
                {{ line.text }}
              </div>
              <div v-else-if="line.kind === 'meta'" class="px-3 text-muted-foreground select-none">
                {{ line.text }}
              </div>
              <button
                v-else-if="line.kind === 'fold'"
                class="block w-full bg-muted/40 select-none hover:bg-accent"
                :title="t('git.graph.detail.diffExpand', { count: foldCountOf(line) })"
                @click="expandFold(foldKeyOf(line))"
              >
                <div class="diff-fold-wave mx-3 h-5" />
              </button>
              <div
                v-else
                class="flex w-full"
                :class="
                  line.kind === 'add'
                    ? 'bg-green-500/10'
                    : line.kind === 'del'
                      ? 'bg-red-500/10'
                      : ''
                "
              >
                <span class="w-10 shrink-0 pr-2 text-right text-muted-foreground/50 select-none">
                  {{ line.oldLine ?? "" }}
                </span>
                <span class="w-10 shrink-0 pr-2 text-right text-muted-foreground/50 select-none">
                  {{ line.newLine ?? "" }}
                </span>
                <span class="whitespace-pre">
                  <template v-if="hlOf(line)">{{ line.text.charAt(0) }}</template>
                  <span v-if="hlOf(line)" class="diff-hl" v-html="hlOf(line)" />
                  <template v-else>{{ line.text }}</template>
                </span>
              </div>
            </template>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* shiki 双主题产物只在 token span 上留 --shiki-light/--shiki-dark 变量,这里按 .dark 切换实际用哪组;
   token 经 v-html 注入没有 scoped 属性,选择器整段包 :global()(同 CommandEditor 的写法) */
:global(.commit-diff .diff-hl span) {
  color: var(--shiki-light);
}

:global(html.dark .commit-diff .diff-hl span) {
  color: var(--shiki-dark);
}

/* 折叠占位行的波浪线:SVG data-uri 平铺,background 文档隔离无法用 currentColor,取中性灰适配亮暗主题 */
.diff-fold-wave {
  background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='6'%3E%3Cpath d='M0 3 Q3 0.5 6 3 T12 3' fill='none' stroke='%239ca3af' stroke-width='1.2'/%3E%3C/svg%3E");
  background-repeat: repeat-x;
  background-position: center;
}
</style>
