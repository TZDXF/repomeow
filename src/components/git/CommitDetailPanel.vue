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
  Eraser,
  ExternalLink,
  FoldVertical,
  FolderTree,
  GitBranch,
  Highlighter,
  List,
  Loader2,
  PanelRightClose,
  Rows2,
  Tag as TagIcon,
} from "@lucide/vue";
import { Icon } from "@iconify/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  intralineRanges,
  parseDiff,
  toSideBySideRows,
  type DiffFold,
  type DiffLine,
} from "@/lib/diff";
import { emphasisTextHtml, highlightDiffLines, wordClsOf } from "@/lib/diff-highlight";
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

/** 忽略空白差异模式(持久化):none 不忽略 / eol 行尾 / change 空白数量变化 / all 全部空白 */
const ignoreWs = useLocalStorage<"none" | "eol" | "change" | "all">(
  "repomeow:commit-diff-ignore-ws",
  "none",
);
/** 行内差异高亮(持久化):成对增删行中实际不同的片段加强底色 */
const wordDiff = useLocalStorage("repomeow:commit-diff-word", true);

/** 行内差异区间(wordDiff 开启时按当前 diff 行集计算;shiki 未着色的行回退渲染也用它) */
const wordRanges = shallowRef(new Map<DiffLine, [number, number]>());
/** 着色并发序号:连切文件/连按开关时,旧的着色结果不得覆盖新的 */
let highlightSeq = 0;

/** diff 行模型:不是 computed——由 selectFile 在着色完成后与 diff / lineHtml 同帧写入(见 selectFile 注释)。
 *  必须声明在 loadFiles 之前:watch immediate 会在 setup 阶段同步调用 loadFiles,声明在后会触发 TDZ 报错 */
const diffLines = shallowRef<DiffLine[]>([]);

/** 逐行着色结果:DiffLine → 行内 HTML;键就是 diffLines 里的对象引用(fold/sideRows 复用同一批) */
const lineHtml = shallowRef(new Map<DiffLine, string>());

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

async function selectFile(file: GitCommitFile, force = false) {
  if (!force && selectedPath.value === file.path && diff.value) {
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
      ignoreWs: ignoreWs.value === "none" ? null : ignoreWs.value,
    });
    // 快速连点 A→B:旧响应解析后可能晚于新请求的 selectedPath,
    // 不做 stale 校验会把 A 的 diff 写到 B 的标题下,先于 B 的真实结果
    if (selectedPath.value !== file.path) return;
    // 先解析再着色,完成后 diff / 行模型 / 高亮同一帧落地:避免纯文本先闪现再上色。
    // 行模型直接复用这份 parseDiff 结果,保证 lineHtml 的键与模板渲染的是同一批对象引用;
    // 折叠/滚动位置属旧 diff,也等这一帧一起重置
    const lines = parseDiff(result.diff);
    const ranges = wordDiff.value
      ? intralineRanges(lines)
      : new Map<DiffLine, [number, number]>();
    const htmlMap =
      (await highlightDiffLines(lines, file.path, ranges)) ?? new Map<DiffLine, string>();
    if (selectedPath.value !== file.path) return;
    highlightSeq++;
    wordRanges.value = ranges;
    lineHtml.value = htmlMap;
    diffLines.value = lines;
    diff.value = result;
    expandedFolds.value = new Set();
    currentScrollTop.value = 0;
    currentRowPos.value = 0;
  } catch (e) {
    if (selectedPath.value !== file.path) return;
    diffError.value = String(e);
  } finally {
    if (selectedPath.value === file.path) diffLoading.value = false;
  }
}

// 忽略空白模式变化:按新模式重取当前文件 diff(行集会变,折叠/滚动位置随 selectFile 一并重置)
watch(ignoreWs, () => {
  if (selectedFile.value) void selectFile(selectedFile.value, true);
});

/** 仅重算行内区间并重新着色(行内高亮开关用,不重新取 diff) */
async function rehighlight(filePath: string, ranges: Map<DiffLine, [number, number]>) {
  const seq = ++highlightSeq;
  const htmlMap =
    (await highlightDiffLines(diffLines.value, filePath, ranges)) ?? new Map<DiffLine, string>();
  if (seq !== highlightSeq || selectedPath.value !== filePath) return;
  wordRanges.value = ranges;
  lineHtml.value = htmlMap;
}

watch(wordDiff, () => {
  const path = selectedPath.value;
  if (!path || !diffLines.value.length) return;
  const ranges = wordDiff.value
    ? intralineRanges(diffLines.value)
    : new Map<DiffLine, [number, number]>();
  void rehighlight(path, ranges);
});

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

/** 模板取行内 HTML:优先 shiki 着色结果;未着色但有行内差异区间的行退化为转义纯文本 + 差异底色 */
function hlOf(line: DiffLine | null | undefined) {
  if (!line) return "";
  const html = lineHtml.value.get(line);
  if (html) return html;
  const range = wordRanges.value.get(line);
  return range ? emphasisTextHtml(line.text.slice(1), range, wordClsOf(line)) : "";
}

/** 并排查看(持久化):旧版本在左、新版本在右 */
const splitDiff = useLocalStorage("repomeow:commit-diff-split", false);
const sideRows = computed(() => toSideBySideRows(displayLines.value));

/** 并排左右内容窗格的宽度比(持久化,拖拽中间连接条调整;0.5 = 各占一半) */
const splitRatio = useLocalStorage("repomeow:commit-diff-split-ratio", 0.5);
const SPLIT_RATIO_MIN = 0.2;
const SPLIT_RATIO_MAX = 0.8;
/** 并排视图整行容器(左内容 | 左行号 | 连接条 | 右行号 | 右内容),拖拽换算宽度比的基准 */
const splitWrapEl = ref<HTMLElement | null>(null);

/** 并排视图去掉行首 diff 标记(+ / - / 空格) */
function sideText(line: DiffLine | null) {
  return line ? line.text.slice(1) : "";
}

// --- 并排视图行模型:对齐 IntelliJ/rebased 默认并排 diff(Align Changes 关闭)——
// 一侧没有对应内容时不插入占位,两侧各自连续渲染本侧行,行号也随各自窗格走;
// 滚动同步不靠等高镜像,而是经 paneRowOffsets 把滚动位置映射到 sideRow 空间再映回对侧 ---
interface PaneRow {
  kind: "hunk" | "meta" | "line" | "fold";
  text: string;
  line: DiffLine | null;
  /** fold: 折叠的行数 */
  count: number;
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
        foldKey: row.fold?.key ?? "",
      });
      continue;
    }
    if (row.kind !== "line") {
      out.push({ kind: row.kind, text: row.text, line: null, count: 0, foldKey: "" });
      continue;
    }
    const line = side === "left" ? row.left : row.right;
    // 本侧无对应内容:不占位,直接跳过(IntelliJ 默认并排 diff 行为)
    if (line) {
      out.push({ kind: "line", text: "", line, count: 0, foldKey: "" });
    }
  }
  return out;
}

const leftRows = computed(() => buildPaneRows("left"));
const rightRows = computed(() => buildPaneRows("right"));

/** 各 sideRow 在左/右窗格内容中的起始行偏移(单位:行,行高 1.25rem 全行统一;末尾带总量哨兵) */
const paneRowOffsets = computed(() => {
  const left: number[] = [];
  const right: number[] = [];
  let l = 0;
  let r = 0;
  for (const row of sideRows.value) {
    left.push(l);
    right.push(r);
    if (row.kind === "line") {
      if (row.left) l++;
      if (row.right) r++;
    } else {
      // hunk/meta/fold 两侧同高
      l++;
      r++;
    }
  }
  left.push(l);
  right.push(r);
  return { left, right };
});

// --- 并排视图滚动:左右内容窗格 + 各自行号栏共四栏;同侧行号栏与内容窗格等高直接镜像,
// 两侧之间经 sideRow 空间锚点映射(变更块顶对齐、块内短侧停在块首,对齐 IntelliJ 的同步滚动);
// 行号栏无横向滚动,横向只在两内容窗格间同步 ---
const leftPaneEl = ref<HTMLElement | null>(null);
const rightPaneEl = ref<HTMLElement | null>(null);
const leftGutterEl = ref<HTMLElement | null>(null);
const rightGutterEl = ref<HTMLElement | null>(null);
// per-instance 状态(用 ref 避免被 <script setup> 模块作用域共享,多实例并发时互不干扰)
const paneSyncing = ref(false);
let paneSyncFrame = 0;

function visualScrollLeft(el: HTMLElement) {
  return el === leftPaneEl.value ? el.scrollWidth - el.clientWidth - el.scrollLeft : el.scrollLeft;
}

function applyVisualScrollLeft(el: HTMLElement, offset: number) {
  el.scrollLeft = el === leftPaneEl.value ? el.scrollWidth - el.clientWidth - offset : offset;
}

/** 滚动位置(px)→ sideRow 空间的小数行位置(跨两侧统一的规范坐标) */
function locateRowPos(side: "left" | "right", scrollTop: number) {
  const offsets = paneRowOffsets.value[side];
  if (offsets.length < 2) return 0;
  const h = rowHeightPx();
  const total = offsets[offsets.length - 1];
  const s = Math.min(Math.max((scrollTop - h / 5) / h, 0), total);
  let lo = 0;
  let hi = offsets.length - 2;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (offsets[mid] <= s) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  const segLen = offsets[lo + 1] - offsets[lo];
  return lo + (segLen > 0 ? (s - offsets[lo]) / segLen : 0);
}

/** sideRow 空间的小数行位置 → 该侧窗格的 scrollTop(px;内容容器有 py-1 上内边距 = 行高的 1/5) */
function scrollTopAt(side: "left" | "right", rowPos: number) {
  const offsets = paneRowOffsets.value[side];
  if (offsets.length < 2) return 0;
  const h = rowHeightPx();
  const i = Math.min(Math.max(Math.floor(rowPos), 0), offsets.length - 2);
  const f = Math.min(Math.max(rowPos - i, 0), 1);
  return (offsets[i] + f * (offsets[i + 1] - offsets[i])) * h + h / 5;
}

function syncPaneScroll(source: "left" | "right" | "leftGutter" | "rightGutter") {
  // 连接条坐标按两窗格实际 scrollTop 刷新,先于重入锁判断:被锁拦截的事件
  // (对侧程序性滚动的回火)同样刷新,拖拽滚动条/连续滚轮时图形实时跟随不滞后
  if (leftPaneEl.value) leftScrollPx.value = leftPaneEl.value.scrollTop;
  if (rightPaneEl.value) rightScrollPx.value = rightPaneEl.value.scrollTop;
  if (paneSyncing.value) return;
  const side = source === "left" || source === "leftGutter" ? "left" : "right";
  const other = side === "left" ? "right" : "left";
  const panes = { left: leftPaneEl, right: rightPaneEl };
  const gutters = { left: leftGutterEl, right: rightGutterEl };
  const fromGutter = source === "leftGutter" || source === "rightGutter";
  const from = (fromGutter ? gutters[side] : panes[side]).value;
  if (!from) return;
  const pos = locateRowPos(side, from.scrollTop);
  currentRowPos.value = pos;
  paneSyncing.value = true;
  // 同侧行号栏 ↔ 内容窗格:等高镜像
  const mate = (fromGutter ? panes[side] : gutters[side]).value;
  if (mate) mate.scrollTop = from.scrollTop;
  // 对侧:经 sideRow 空间锚点映射
  const mapped = scrollTopAt(other, pos);
  if (panes[other].value) panes[other].value.scrollTop = mapped;
  if (gutters[other].value) gutters[other].value.scrollTop = mapped;
  if (!fromGutter && panes[other].value) {
    applyVisualScrollLeft(panes[other].value, visualScrollLeft(from));
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

// --- 中间连接条(IntelliJ divider 风格):变更块画彩色多边形(红删/绿增/蓝改),
// 折叠行画波浪色的连接线;左右纵边分别贴两侧窗格中块的 viewport 纵坐标,滚动时各自跟随 ---
const dividerEl = ref<HTMLElement | null>(null);
const { width: dividerWidth, height: dividerHeight } = useElementSize(dividerEl);
const leftScrollPx = ref(0);
const rightScrollPx = ref(0);

interface DividerShape {
  kind: "poly" | "line";
  /** poly: path d(px,相对连接条左上角;左右纵边之间以三次贝塞尔曲线过渡) */
  d: string;
  /** line: 两端 viewport 纵坐标 */
  y1: number;
  y2: number;
  cls: string;
}

/** 变更块:sideRow 空间的连续增删段(连接条多边形与空白侧插入位置线共用) */
interface ChangeBlock {
  start: number;
  end: number;
  cls: "divider-del" | "divider-add" | "divider-mod";
  /** 左/右侧在该块内是否有内容行(无内容侧画插入位置线) */
  hasLeft: boolean;
  hasRight: boolean;
}

const changeBlocks = computed(() => {
  const rows = sideRows.value;
  const blocks: ChangeBlock[] = [];
  const isChange = (idx: number) => {
    const row = rows[idx];
    return row.kind === "line" && (row.left?.kind === "del" || row.right?.kind === "add");
  };
  let i = 0;
  while (i < rows.length) {
    if (!isChange(i)) {
      i++;
      continue;
    }
    let j = i;
    let hasDel = false;
    let hasAdd = false;
    while (j < rows.length && isChange(j)) {
      if (rows[j].left?.kind === "del") hasDel = true;
      if (rows[j].right?.kind === "add") hasAdd = true;
      j++;
    }
    blocks.push({
      start: i,
      end: j,
      cls: hasDel && hasAdd ? "divider-mod" : hasDel ? "divider-del" : "divider-add",
      hasLeft: hasDel,
      hasRight: hasAdd,
    });
    i = j;
  }
  return blocks;
});

/**
 * 空白侧插入位置线:本侧无内容的变更块,在块应处的行缝画一条变更色细线。
 * top 为内容坐标(行,乘 1.25rem 行高),标记渲染在窗格/行号栏内容里随滚动走,
 * 顶端与连接条多边形塌缩的顶点同一纵坐标,连成一线
 */
function insertMarkersOf(side: "left" | "right") {
  const has = side === "left" ? "hasLeft" : "hasRight";
  return changeBlocks.value
    .filter((b) => !b[has])
    .map((b) => ({
      top: paneRowOffsets.value[side][b.start],
      cls: b.hasRight ? "insert-add" : "insert-del",
    }));
}

const leftMarkers = computed(() => insertMarkersOf("left"));
const rightMarkers = computed(() => insertMarkersOf("right"));

const dividerShapes = computed(() => {
  const w = dividerWidth.value || 20;
  const vh = dividerHeight.value;
  const lScroll = leftScrollPx.value;
  const rScroll = rightScrollPx.value;
  const h = rowHeightPx();
  const yL = (pos: number) => scrollTopAt("left", pos) - lScroll;
  const yR = (pos: number) => scrollTopAt("right", pos) - rScroll;
  const shapes: DividerShape[] = [];
  const rows = sideRows.value;
  rows.forEach((row, idx) => {
    // 折叠行:两侧 fold 行中点连线(未对齐时呈斜线,与 IntelliJ 连接条观感一致)
    if (row.kind !== "fold") return;
    const ly = yL(idx) + h / 2;
    const ry = yR(idx) + h / 2;
    if (ly >= -h && ry >= -h && (ly <= vh + h || ry <= vh + h)) {
      shapes.push({ kind: "line", d: "", y1: ly, y2: ry, cls: "divider-fold" });
    }
  });
  for (const block of changeBlocks.value) {
    // 左右纵边 = 块首/末行在各自窗格的位置(空侧塌缩成点,顶点接插入位置线);
    // 纵边之间用控制点在中点的三次贝塞尔(S 形曲线)过渡,比直线多边形更柔和
    const lTop = yL(block.start);
    const lBottom = yL(block.end);
    const rTop = yR(block.start);
    const rBottom = yR(block.end);
    // 视口外(上下各留一行余量)跳过,避免大 diff 生成几千个路径
    if ((lBottom >= -h || rBottom >= -h) && (lTop <= vh + h || rTop <= vh + h)) {
      const mid = w / 2;
      shapes.push({
        kind: "poly",
        d: `M0,${lTop} C${mid},${lTop} ${mid},${rTop} ${w},${rTop} L${w},${rBottom} C${mid},${rBottom} ${mid},${lBottom} 0,${lBottom} Z`,
        y1: 0,
        y2: 0,
        cls: block.cls,
      });
    }
  }
  return shapes;
});

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

// 布局切换(上下 ↔ 左右)后:旧滚动位置可能不再对应新容器的内容位置,
// 直接归零让 chevron 上/下按钮按新视图的"顶端"重新判定 hasPrev/Next;
/// syncHbarPad 已经在 watch 列表里,这里只补滚动位置
watch(layout, () => {
  currentScrollTop.value = 0;
  currentRowPos.value = 0;
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

// --- 行号栏横向滚动条补偿:内容窗格出现横向滚动条时可滚范围多出一个滚动条高度,
// 同侧行号栏与内容窗格是等高镜像,给行号栏补等量底部内边距拉平可滚范围
// (两侧窗格之间走 sideRow 空间映射,无需互相补齐) ---
const hbarPad = ref({ leftGutter: 0, rightGutter: 0 });

async function syncHbarPad() {
  await nextTick();
  hbarPad.value = {
    leftGutter: leftPaneEl.value ? leftPaneEl.value.offsetHeight - leftPaneEl.value.clientHeight : 0,
    rightGutter: rightPaneEl.value
      ? rightPaneEl.value.offsetHeight - rightPaneEl.value.clientHeight
      : 0,
  };
}

// 内容 / 布局 / 面板与列表宽度变化都可能改变横向滚动条的出现与否(连接条拖拽改两侧窗格宽度同理)
watch(
  [displayLines, splitActive, panelWidth, listWidth, layout, splitRatio],
  () => void syncHbarPad(),
  {
    flush: "post",
  },
);

// --- 差异导航:上/下一个差异;逐行模式按 px(行索引 × 行高 + py-1 上内边距),
// 并排模式按 sideRow 空间行位置(经 scrollTopAt 换算到两侧窗格) ---
const unifiedEl = ref<HTMLElement | null>(null);
const currentScrollTop = ref(0);
/** 并排模式当前滚动位置(sideRow 空间的小数行,与 scrollTopAt/locateRowPos 同坐标系) */
const currentRowPos = ref(0);

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

/** 差异块顶部位置:逐行为 scrollTop(px),并排为 sideRow 空间行位置 */
function changeOffsets() {
  if (splitActive.value) return changeRowIdx.value;
  const h = rowHeightPx();
  return changeRowIdx.value.map((i) => h / 5 + i * h);
}

const hasPrevChange = computed(() =>
  splitActive.value
    ? changeOffsets().some((o) => o < currentRowPos.value - 0.1)
    : changeOffsets().some((o) => o < currentScrollTop.value - 2),
);
const hasNextChange = computed(() =>
  splitActive.value
    ? changeOffsets().some((o) => o > currentRowPos.value + 0.1)
    : changeOffsets().some((o) => o > currentScrollTop.value + 2),
);

function scrollToChange(dir: 1 | -1) {
  const offsets = changeOffsets();
  if (splitActive.value) {
    const cur = currentRowPos.value;
    const target =
      dir === 1
        ? offsets.find((o) => o > cur + 0.1)
        : [...offsets].reverse().find((o) => o < cur - 0.1);
    if (target == null) return;
    currentRowPos.value = target;
    // 直接给各栏赋值,不等 scroll 事件传播
    const left = scrollTopAt("left", target);
    const right = scrollTopAt("right", target);
    if (leftPaneEl.value) leftPaneEl.value.scrollTop = left;
    if (leftGutterEl.value) leftGutterEl.value.scrollTop = left;
    if (rightPaneEl.value) rightPaneEl.value.scrollTop = right;
    if (rightGutterEl.value) rightGutterEl.value.scrollTop = right;
    return;
  }
  const cur = currentScrollTop.value;
  const target =
    dir === 1 ? offsets.find((o) => o > cur + 2) : [...offsets].reverse().find((o) => o < cur - 2);
  if (target == null) return;
  currentScrollTop.value = target;
  if (unifiedEl.value) unifiedEl.value.scrollTop = target;
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

// --- 中间连接条拖拽:调整并排左右内容窗格的宽度比 ---
// 连接条中心相对整行容器定位,扣除两侧行号栏与连接条本身宽度后换算成比值
function startSplitResize(e: PointerEvent) {
  e.preventDefault();
  const wrap = splitWrapEl.value;
  const lg = leftGutterEl.value;
  const rg = rightGutterEl.value;
  const dv = dividerEl.value;
  if (!wrap || !lg || !rg || !dv) return;
  const paneArea = wrap.clientWidth - lg.offsetWidth - rg.offsetWidth - dv.offsetWidth;
  if (paneArea <= 0) return;
  const baseX = wrap.getBoundingClientRect().left + lg.offsetWidth + dv.offsetWidth / 2;
  const onMove = (ev: PointerEvent) => {
    splitRatio.value = Math.min(
      SPLIT_RATIO_MAX,
      Math.max(SPLIT_RATIO_MIN, (ev.clientX - baseX) / paneArea),
    );
  };
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    listResizeCleanups = listResizeCleanups.filter((fn) => fn !== onUp);
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
  listResizeCleanups.push(onUp);
}

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
          <DropdownMenu v-if="diff">
            <DropdownMenuTrigger as-child>
              <button
                class="shrink-0 rounded-sm p-1 transition-colors hover:bg-accent hover:text-foreground"
                :class="ignoreWs !== 'none' ? 'bg-accent text-foreground' : 'text-muted-foreground'"
                :title="t('git.graph.detail.diffIgnoreWs')"
              >
                <Eraser class="h-3.5 w-3.5" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" class="w-auto whitespace-nowrap">
              <DropdownMenuRadioGroup v-model="ignoreWs">
                <DropdownMenuRadioItem value="none">
                  {{ t("git.graph.detail.diffIgnoreWsNone") }}
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="eol">
                  {{ t("git.graph.detail.diffIgnoreWsEol") }}
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="change">
                  {{ t("git.graph.detail.diffIgnoreWsChange") }}
                </DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="all">
                  {{ t("git.graph.detail.diffIgnoreWsAll") }}
                </DropdownMenuRadioItem>
              </DropdownMenuRadioGroup>
            </DropdownMenuContent>
          </DropdownMenu>
          <button
            v-if="diff"
            class="shrink-0 rounded-sm p-1 transition-colors hover:bg-accent hover:text-foreground"
            :class="wordDiff ? 'bg-accent text-foreground' : 'text-muted-foreground'"
            :title="t('git.graph.detail.diffWordHl')"
            @click="wordDiff = !wordDiff"
          >
            <Highlighter class="h-3.5 w-3.5" />
          </button>
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

        <!-- 并排(split)视图:左内容 | 左行号 | 连接条 | 右行号 | 右内容;同侧两栏镜像滚动,两侧经 sideRow 空间锚点映射同步;
             左右内容窗格宽度按 splitRatio(连接条拖拽调整)分配 -->
        <div
          v-if="splitActive && !diffLoading && !diffError && selectedPath"
          ref="splitWrapEl"
          class="flex min-h-0 flex-1"
        >
          <!-- 左窗格 -scale-x-100 双翻转:容器翻转把纵向滚动条移到左边,内容层再翻转回正;
               代价是 scrollLeft 镜像化(0=内容末尾、最大值=行首),横向同步经 visualScrollLeft/applyVisualScrollLeft 换算 -->
          <div
            ref="leftPaneEl"
            class="min-w-0 -scale-x-100 overflow-auto"
            :style="{ flex: `${splitRatio} 1 0%` }"
            @scroll="syncPaneScroll('left')"
            @wheel="onLeftPaneWheel"
          >
            <div class="diff-code relative min-w-max -scale-x-100 py-1 text-xs leading-5">
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
                  <div class="diff-fold-wave h-5" />
                </button>
                <!-- 行高固定 h-5:去掉行首标记后空行内容为空,行盒会塌缩成 0 高导致与行号栏错位 -->
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
              <!-- 空白侧插入位置线:对侧纯新增块在本侧的行缝标记,顶端接连接条多边形顶点 -->
              <div
                v-for="(m, i) in leftMarkers"
                :key="i"
                class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
                :class="m.cls"
                :style="{ top: `calc(0.25rem + ${m.top * 1.25}rem)` }"
              />
            </div>
          </div>

          <!-- 左侧行号栏:旧版本(del/ctx)行号,与左内容窗格等高镜像滚动;横向不滚,纵向滚动条隐藏但可滚 -->
          <div
            ref="leftGutterEl"
            class="shrink-0 overflow-auto border-l border-border/60 font-mono text-xs leading-5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
            @scroll="syncPaneScroll('leftGutter')"
          >
            <div
              class="relative py-1"
              :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.leftGutter}px)` }"
            >
              <template v-for="(row, i) in leftRows" :key="i">
                <div v-if="row.kind === 'hunk'" class="h-5 bg-muted/60 select-none" />
                <div v-else-if="row.kind === 'meta'" class="h-5 select-none" />
                <!-- 波浪线贯穿行号栏,与内容窗格的折叠行连成一线;同样可点击展开 -->
                <button
                  v-else-if="row.kind === 'fold'"
                  class="block h-5 w-full bg-muted/40 select-none hover:bg-accent"
                  :title="t('git.graph.detail.diffExpand', { count: row.count })"
                  @click="expandFold(row.foldKey)"
                >
                  <div class="diff-fold-wave h-full" />
                </button>
                <div
                  v-else
                  class="h-5 w-10 text-center text-muted-foreground/50 select-none"
                  :class="row.line?.kind === 'del' ? 'bg-red-500/10' : ''"
                >
                  {{ row.line?.oldLine ?? "" }}
                </div>
              </template>
              <div
                v-for="(m, i) in leftMarkers"
                :key="i"
                class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
                :class="m.cls"
                :style="{ top: `calc(0.25rem + ${m.top * 1.25}rem)` }"
              />
            </div>
          </div>

          <!-- 中间连接条:变更块曲线带 + 折叠波浪连接线(IntelliJ divider 风格);可拖拽调整左右窗格宽度比 -->
          <div
            ref="dividerEl"
            class="relative w-5 shrink-0 cursor-col-resize select-none transition-colors hover:bg-primary/40"
            @pointerdown="startSplitResize"
          >
            <svg class="pointer-events-none absolute inset-0 h-full w-full">
              <template v-for="(shape, i) in dividerShapes" :key="i">
                <path v-if="shape.kind === 'poly'" :d="shape.d" :class="shape.cls" />
                <line v-else x1="0" :y1="shape.y1" x2="100%" :y2="shape.y2" :class="shape.cls" />
              </template>
            </svg>
          </div>

          <!-- 右侧行号栏:新版本(add/ctx)行号,与右内容窗格等高镜像滚动 -->
          <div
            ref="rightGutterEl"
            class="shrink-0 overflow-auto border-r border-border/60 font-mono text-xs leading-5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
            @scroll="syncPaneScroll('rightGutter')"
          >
            <div
              class="relative py-1"
              :style="{ paddingBottom: `calc(0.25rem + ${hbarPad.rightGutter}px)` }"
            >
              <template v-for="(row, i) in rightRows" :key="i">
                <div v-if="row.kind === 'hunk'" class="h-5 bg-muted/60 select-none" />
                <div v-else-if="row.kind === 'meta'" class="h-5 select-none" />
                <button
                  v-else-if="row.kind === 'fold'"
                  class="block h-5 w-full bg-muted/40 select-none hover:bg-accent"
                  :title="t('git.graph.detail.diffExpand', { count: row.count })"
                  @click="expandFold(row.foldKey)"
                >
                  <div class="diff-fold-wave h-full" />
                </button>
                <div
                  v-else
                  class="h-5 w-10 text-center text-muted-foreground/50 select-none"
                  :class="row.line?.kind === 'add' ? 'bg-green-500/10' : ''"
                >
                  {{ row.line?.newLine ?? "" }}
                </div>
              </template>
              <div
                v-for="(m, i) in rightMarkers"
                :key="i"
                class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
                :class="m.cls"
                :style="{ top: `calc(0.25rem + ${m.top * 1.25}rem)` }"
              />
            </div>
          </div>

          <div
            ref="rightPaneEl"
            class="min-w-0 overflow-auto"
            :style="{ flex: `${1 - splitRatio} 1 0%` }"
            @scroll="syncPaneScroll('right')"
          >
            <div class="diff-code relative min-w-max py-1 text-xs leading-5">
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
                  <div class="diff-fold-wave h-5" />
                </button>
                <!-- 行高固定 h-5:去掉行首标记后空行内容为空,行盒会塌缩成 0 高导致与行号栏错位 -->
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
              <!-- 空白侧插入位置线:对侧纯删除块在本侧的行缝标记,顶端接连接条多边形顶点 -->
              <div
                v-for="(m, i) in rightMarkers"
                :key="i"
                class="pointer-events-none absolute inset-x-0 h-0.5 -translate-y-1/2"
                :class="m.cls"
                :style="{ top: `calc(0.25rem + ${m.top * 1.25}rem)` }"
              />
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
          <div v-else class="diff-code min-w-max py-1 text-xs leading-5">
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
/* diff 代码字体栈:Tailwind font-mono 以泛型 monospace 收尾,WebView2(zh-CN)按「等宽语境」
   把栈内没有的 CJK 字符回退到系统中文等宽默认 NSimSun(宋体系),与 Consolas 的 ASCII 观感割裂;
   在泛型前显式插入微软雅黑,汉字统一走雅黑。ASCII 仍按原栈取 Consolas(macOS 则前置条目取 SF Mono/Menlo) */
.diff-code {
  font-family:
    ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New",
    "Microsoft YaHei", monospace;
}

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

/* 中间连接条:变更块曲线带(低透明填充 + 同色描边,拐角圆滑)与折叠连接线(取波浪线同款中性灰) */
.divider-del,
.divider-add,
.divider-mod {
  stroke-linejoin: round;
}

.divider-del {
  fill: rgb(239 68 68 / 0.16);
  stroke: rgb(239 68 68 / 0.45);
  stroke-width: 1;
}

.divider-add {
  fill: rgb(34 197 94 / 0.16);
  stroke: rgb(34 197 94 / 0.45);
  stroke-width: 1;
}

.divider-mod {
  fill: rgb(59 130 246 / 0.16);
  stroke: rgb(59 130 246 / 0.45);
  stroke-width: 1;
}

.divider-fold {
  stroke: #9ca3af;
  stroke-width: 1.2;
}

/* 空白侧插入位置线:本侧无内容的变更块,在块应处的行缝画一条变更色细线(贯穿窗格与行号栏) */
.insert-add {
  background-color: rgb(34 197 94 / 0.55);
}

.insert-del {
  background-color: rgb(239 68 68 / 0.55);
}

/* 行内差异底色:成对增删行中实际不同的片段;套在 shiki token span 外层,只加背景不改文字色。
   token 经 v-html 注入没有 scoped 属性,选择器整段包 :global()(同 .diff-hl 的写法) */
:global(.commit-diff .diff-word-del) {
  background-color: rgb(239 68 68 / 0.28);
}

:global(.commit-diff .diff-word-add) {
  background-color: rgb(34 197 94 / 0.28);
}
</style>
