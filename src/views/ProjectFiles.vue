<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, provide, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { ArrowLeft, Code, Eye, FileQuestion, FolderTree, Search, WrapText } from "@lucide/vue";
import { onClickOutside, useLocalStorage } from "@vueuse/core";
import { Markdown, type ControlsConfig, type NodeRenderers } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import CodeViewer from "@/components/files/CodeViewer.vue";
import FindBar from "@/components/files/FindBar.vue";
import TextSearchPanel from "@/components/files/TextSearchPanel.vue";
import FileTreeList from "@/components/common/FileTreeList.vue";
import MdImage from "@/components/markdown/MdImage.vue";
import MdLink from "@/components/markdown/MdLink.vue";
import { MD_BASE_PATH_KEY } from "@/components/markdown/keys";
import ImageViewer from "@/components/files/ImageViewer.vue";
import { cmd } from "@/lib/tauri";
import { debounce } from "@/lib/utils";
import { extOf, IMAGE_EXTS } from "@/lib/file-kind";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { buildVisibleRows, prefetchTargets, sortDirEntries } from "@/lib/lazy-file-tree";
import { fileIcon } from "@/lib/file-icons";
import { buildFindRegExp, type FindQuery } from "@/lib/text-search";
import { Icon } from "@iconify/vue";
import { useSettingsStore } from "@/stores/settings";
import { useProjectsStore } from "@/stores/projects";
import type { FilePreview, Project, ProjectFileEntry } from "@/types";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const store = useProjectsStore();
const settingsStore = useSettingsStore();

const project = computed<Project | undefined>(() => {
  const id = Number(route.params.id);
  return Number.isFinite(id) ? store.projects.find((p) => p.id === id) : undefined;
});

// ── 工作区跟随:与 ProjectDetail 同一 localStorage 键(projectId -> worktree 绝对路径) ──
// 文件树/预览/搜索全部以当前工作区为根,未选择 worktree 时回退主工作区路径;
// 选中的 worktree 已被删除时由详情页 WorktreeSwitcher 校验并重置该键
const worktreeSelection = useLocalStorage<Record<string, string>>(
  "repomeow.worktree-selection",
  {},
);
const rootPath = computed(() => {
  const id = project.value?.id;
  return (
    (id != null ? worktreeSelection.value[String(id)] : undefined) ?? project.value?.path ?? ""
  );
});

// ── 文件树(逐层懒加载 + 单层后台预取) ───────────────────────────────────────
// childrenMap key 为目录相对路径(根为 ""),值为该层已排序子项;不变的预取策略:
// 任何可见目录行的下一层已加载或在途——点击展开时数据已经就位,无需等待后端
const childrenMap = ref(new Map<string, ProjectFileEntry[]>());
const expandedFolders = ref(new Set<string>());
const listLoading = ref(false);
const listError = ref(false);
let listSeq = 0;
const inflight = new Map<string, Promise<void>>();

const PREFETCH_CONCURRENCY = 4;
const prefetchQueue: string[] = [];
const prefetchQueued = new Set<string>();
let prefetchActive = 0;

async function ensureChildren(dir: string): Promise<void> {
  if (childrenMap.value.has(dir)) {
    return;
  }
  const existing = inflight.get(dir);
  if (existing) {
    return existing;
  }
  if (!project.value) {
    return;
  }
  const path = rootPath.value;
  const seq = listSeq;
  const p = (async () => {
    try {
      const entries = await cmd<ProjectFileEntry[]>("list_project_files", {
        path,
        dir: dir || null,
      });
      if (seq !== listSeq) {
        return;
      }
      const next = new Map(childrenMap.value);
      next.set(dir, sortDirEntries(entries));
      childrenMap.value = next;
      // 目录处于展开 frontier(根或已展开)时,其可见子目录的下一层提前后台就位
      if (dir === "" || expandedFolders.value.has(dir)) prefetchNext(entries);
    } catch {
      if (seq !== listSeq) {
        return;
      }
      if (dir === "") {
        listError.value = true;
      } else {
        // 单层失败按已加载空层处理,避免反复展开打满 IPC;下次刷新可恢复
        const next = new Map(childrenMap.value);
        next.set(dir, []);
        childrenMap.value = next;
      }
    } finally {
      inflight.delete(dir);
    }
  })();
  inflight.set(dir, p);
  return p;
}

function prefetchNext(children: ProjectFileEntry[]) {
  for (const dir of prefetchTargets(children)) {
    if (childrenMap.value.has(dir) || inflight.has(dir) || prefetchQueued.has(dir)) {
      continue;
    }
    prefetchQueued.add(dir);
    prefetchQueue.push(dir);
  }
  pumpPrefetch();
}

function pumpPrefetch() {
  while (prefetchActive < PREFETCH_CONCURRENCY && prefetchQueue.length) {
    const dir = prefetchQueue.shift()!;
    prefetchQueued.delete(dir);
    prefetchActive++;
    void ensureChildren(dir).finally(() => {
      prefetchActive--;
      pumpPrefetch();
    });
  }
}

function toggleFolder(fullPath: string) {
  const children = childrenMap.value.get(fullPath);
  if (children && children.length === 0) {
    return; // 已知空目录无展开意义
  }
  const next = new Set(expandedFolders.value);
  if (next.has(fullPath)) {
    next.delete(fullPath);
  } else {
    next.add(fullPath);
    if (children) {
      // 子层已就位:补孙级预取(兜住子层到达时本目录尚未展开的竞态)
      prefetchNext(children);
    } else {
      // 预取未覆盖(点击快过预取,或不参与预取的排除目录):按需拉取,行内显加载占位
      void ensureChildren(fullPath);
    }
  }
  expandedFolders.value = next;
}

const visibleRows = computed(() => buildVisibleRows(childrenMap.value, expandedFolders.value));

const rootEmpty = computed(
  () => childrenMap.value.has("") && childrenMap.value.get("")!.length === 0,
);

async function loadFiles() {
  if (!project.value) {
    return;
  }
  listSeq++;
  inflight.clear();
  prefetchQueue.length = 0;
  prefetchQueued.clear();
  childrenMap.value = new Map();
  expandedFolders.value = new Set();
  listLoading.value = true;
  listError.value = false;
  try {
    await ensureChildren("");
  } finally {
    listLoading.value = false;
  }
}

// 项目或工作区变化(根路径变化)时整树重载(选中/预览清理见下方预览状态声明处的 watch)
watch(rootPath, () => void loadFiles(), { immediate: true });

// ── 头部文件搜索:右侧按钮触发,顶部中间搜索框 + 下拉选项 ─────────────────────
// 懒加载后前端没有全量清单,搜索下沉后端 search_project_files
// (遍历口径:未被 .gitignore/.ignore 排除的文件,与原「被排除文件不参与」一致)
const FILE_SEARCH_LIMIT = 50;
const fileSearchOpen = ref(false);
const fileSearchText = ref("");
const fileSearchIndex = ref(0);
const fileSearchResults = ref<ProjectFileEntry[]>([]);
const fileSearchLimited = ref(false);
const fileSearchBox = ref<HTMLElement | null>(null);
const fileSearchBtn = ref<InstanceType<typeof Button> | null>(null);
let fileSearchSeq = 0;
/** 文件搜索防抖:输入停 200ms 后查询 */
const debouncedRunFileSearch = debounce((q: string) => void runFileSearch(q), 200);

watch(fileSearchText, () => {
  fileSearchIndex.value = 0;
  const q = fileSearchText.value.trim();
  if (!q) {
    debouncedRunFileSearch.cancel();
    fileSearchResults.value = [];
    fileSearchLimited.value = false;
    return;
  }
  debouncedRunFileSearch(q);
});

async function runFileSearch(q: string) {
  if (!project.value) {
    return;
  }
  const seq = ++fileSearchSeq;
  try {
    const res = await cmd<ProjectFileEntry[]>("search_project_files", {
      path: rootPath.value,
      query: q,
      limit: FILE_SEARCH_LIMIT + 1, // 多取一条判断是否截断
    });
    if (seq !== fileSearchSeq) {
      return;
    }
    fileSearchLimited.value = res.length > FILE_SEARCH_LIMIT;
    fileSearchResults.value = res.slice(0, FILE_SEARCH_LIMIT);
  } catch {
    if (seq !== fileSearchSeq) {
      return;
    }
    fileSearchResults.value = [];
    fileSearchLimited.value = false;
  }
}

onClickOutside(fileSearchBox, closeFileSearch, { ignore: [fileSearchBtn] });

function toggleFileSearch() {
  if (fileSearchOpen.value) {
    closeFileSearch();
    return;
  }
  fileSearchOpen.value = true;
  void nextTick(() => fileSearchBox.value?.querySelector("input")?.focus());
}

function closeFileSearch() {
  fileSearchOpen.value = false;
  fileSearchText.value = "";
  fileSearchIndex.value = 0;
  debouncedRunFileSearch.cancel();
  fileSearchSeq++;
  fileSearchResults.value = [];
  fileSearchLimited.value = false;
}

async function openFileFromSearch(path: string) {
  selected.value = path;
  leftView.value = "tree";
  closeFileSearch();
  // 沿路径确保各级祖先子层加载(互相独立,并行)并展开,让选中项在树中可见
  const segs = path.split("/");
  const prefixes: string[] = [];
  let prefix = "";
  for (let i = 0; i < segs.length - 1; i++) {
    prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
    prefixes.push(prefix);
  }
  await Promise.all(prefixes.map((p) => ensureChildren(p)));
  const next = new Set(expandedFolders.value);
  for (const p of prefixes) {
    next.add(p);
  }
  expandedFolders.value = next;
  void nextTick(() => {
    document.querySelector(".file-row-selected")?.scrollIntoView({ block: "center" });
  });
}

async function onFileSearchKeydown(e: KeyboardEvent) {
  const total = fileSearchResults.value.length;
  if (e.key === "ArrowDown") {
    e.preventDefault();
    if (total) fileSearchIndex.value = (fileSearchIndex.value + 1) % total;
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (total) fileSearchIndex.value = (fileSearchIndex.value - 1 + total) % total;
  } else if (e.key === "Enter") {
    e.preventDefault();
    // 结果经防抖异步到达:Enter 立即补一次查询,避免按到防抖窗口内的旧结果
    debouncedRunFileSearch.cancel();
    const q = fileSearchText.value.trim();
    if (q) {
      await runFileSearch(q);
    }
    const hit = fileSearchResults.value[fileSearchIndex.value] ?? fileSearchResults.value[0];
    if (hit) {
      void openFileFromSearch(hit.path);
    }
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation(); // 不触发行内查找条的全局 Esc 关闭
    closeFileSearch();
  }
}

// ── 选中与预览 ──────────────────────────────────────────────────────────────
const selected = ref<string | null>(null);
const previewError = ref(false);
const previewText = ref<string | null>(null);
// 当前 previewText 所属文件(与 text 同步赋值/清空):区分「选中了什么」与「内容就位没有」,
// 旧文件残留内容不得进新文件的渲染分支(代码→MD 切换闪旧文本即此因)
const previewPath = ref<string | null>(null);
const previewTruncated = ref(false);
const previewBinary = ref(false);
let previewSeq = 0;

// 根路径变化(切项目/切工作区)时清掉选中与在途预览:旧工作区的文件在新根下无意义,
// 序号递增作废旧根的在途读取,防止其内容回填到新工作区视图
watch(rootPath, () => {
  selected.value = null;
  previewSeq++;
});

const MD_EXTS = new Set(["md", "markdown"]);

const selectedExt = computed(() => (selected.value ? extOf(selected.value) : ""));
const isImage = computed(() => IMAGE_EXTS.has(selectedExt.value));
const isMarkdown = computed(() => MD_EXTS.has(selectedExt.value));
const isSvg = computed(() => selectedExt.value === "svg");

// svg 兼具图像与文本两种形态:源码模式下按文本读取走代码视图,其余图片 asset 直显
const svgMode = ref<"preview" | "source">("preview");
const svgSource = computed(() => isSvg.value && svgMode.value === "source");

const imageSrc = computed(() =>
  selected.value && isImage.value && project.value
    ? convertFileSrc(resolvePath(rootPath.value, selected.value))
    : "",
);

// 切换文件不显示 loading:本地读取很快,loading 只会闪烁;
// 保留旧内容直到新内容就位(序号防串台),仅错误/二进制等状态标记随切换即清
watch([selected, svgSource], async ([path]) => {
  const mySeq = ++previewSeq;
  previewError.value = false;
  previewBinary.value = false;
  previewTruncated.value = false;
  if (!path || !project.value) {
    previewText.value = null;
    previewPath.value = null;
    return;
  }
  if (IMAGE_EXTS.has(extOf(path)) && !svgSource.value) {
    previewText.value = null; // 图片走 asset 协议直显,不读内容(svg 源码模式除外)
    previewPath.value = null;
    return;
  }
  try {
    const res = await cmd<FilePreview>("read_file_preview", {
      root: rootPath.value,
      relPath: path,
    });
    if (mySeq !== previewSeq) return;
    if (res.text === null) {
      previewText.value = null;
      previewPath.value = null;
      previewBinary.value = true;
    } else {
      previewText.value = res.text;
      previewPath.value = path;
      previewTruncated.value = res.truncated;
    }
  } catch {
    if (mySeq === previewSeq) {
      previewText.value = null;
      previewPath.value = null;
      previewError.value = true;
    }
  }
});

// ── 代码视图:CodeMirror 只读查看(行号/折叠/语法高亮/换行由 CodeViewer 承担) ───
const mdMode = ref<"rendered" | "source">("rendered");
// 代码自动换行(持久化),经 prop 驱动 CodeViewer 的 lineWrapping 热切换
const codeWrap = useLocalStorage("repomeow:files-code-wrap", false);

/** 是否以代码视图展示(Markdown 渲染模式下不显示代码) */
const codeVisible = computed(
  () => previewText.value !== null && !(isMarkdown.value && mdMode.value === "rendered"),
);

/** Markdown 渲染分支可否使用当前内容:残留内容须本身来自 md 文件——
 *  代码文件的旧文本以 MD 渲染会闪现错内容,等待新内容期间显示空白(md→md 仍保留旧文防闪) */
const mdRenderable = computed(
  () => previewPath.value !== null && MD_EXTS.has(extOf(previewPath.value)),
);

// ── 搜索:左栏全文搜索面板 + 文件内查找条 ─────────────────────────────────────
const codeViewer = ref<InstanceType<typeof CodeViewer> | null>(null);
const findBarRef = ref<InstanceType<typeof FindBar> | null>(null);
const searchPanelRef = ref<InstanceType<typeof TextSearchPanel> | null>(null);

const leftView = ref<"tree" | "search">("tree");

const findOpen = ref(false);
const findText = ref("");
const findCase = ref(false);
const findWord = ref(false);
const findRegex = ref(false);
const findTotal = ref(0);
const findIndex = ref(-1);

const findQuery = computed<FindQuery>(() => ({
  text: findText.value,
  caseSensitive: findCase.value,
  wholeWord: findWord.value,
  useRegex: findRegex.value,
}));

const findInvalid = computed(() => {
  if (!findRegex.value || !findText.value.trim()) return false;
  return (
    buildFindRegExp({
      text: findText.value,
      caseSensitive: true,
      wholeWord: true,
      useRegex: true,
    }) === null
  );
});

function refreshFind(scrollToCurrent: boolean) {
  const cv = codeViewer.value;
  if (!cv) return;
  const ranges = cv.runFind(findQuery.value);
  findTotal.value = ranges.length;
  findIndex.value = cv.getFindCursor();
  if (scrollToCurrent && ranges.length) findIndex.value = cv.gotoMatch(cv.getFindCursor());
}

// 输入/模式变化即重查;文档就位或切换(切文件)后若查找条仍开着则重跑
watch([findText, findCase, findWord, findRegex], () => {
  if (findOpen.value) refreshFind(true);
});
// 文档就位或切换(切文件)后若查找条仍开着则在就位的新文档上重查;
// 必须 post-flush:pre 会在 CodeViewer 内部 watch(props.text) 换文档之前执行,
// 跑在旧文档上得到上一个文件的结果;post 也保证 MD 渲染切源码时跑在新挂载实例上
watch(
  [codeVisible, previewText],
  () => {
    if (findOpen.value && codeVisible.value) refreshFind(false);
  },
  { flush: "post" },
);

function findStep(delta: number) {
  if (!findTotal.value) return;
  findIndex.value = codeViewer.value?.gotoMatch(findIndex.value + delta) ?? -1;
}

function onFindToggle(key: "caseSensitive" | "wholeWord" | "useRegex") {
  if (key === "caseSensitive") findCase.value = !findCase.value;
  else if (key === "wholeWord") findWord.value = !findWord.value;
  else findRegex.value = !findRegex.value;
}

function openFind() {
  if (!codeVisible.value) return;
  findOpen.value = true;
  refreshFind(true);
  findBarRef.value?.focusInput();
}

function closeFind() {
  findOpen.value = false;
  findTotal.value = 0;
  findIndex.value = -1;
  codeViewer.value?.clearFind();
}

// ── 全文搜索结果跳转:打开文件并定位到命中行 ──────────────────────────────────
const pendingJump = ref<{ path: string; line: number; query: FindQuery } | null>(null);

function onSearchOpen(path: string, line: number, query: FindQuery) {
  selected.value = path;
  // 渲染态(Markdown/SVG)没有代码视图,强制源码模式保证可定位
  if (MD_EXTS.has(extOf(path))) mdMode.value = "source";
  if (extOf(path) === "svg") svgMode.value = "source";
  pendingJump.value = { path, line, query };
  tryJump();
}

function tryJump() {
  const j = pendingJump.value;
  // 目标文件内容就位(previewPath 对上)才跳,防止在残留文本上定位到错误位置
  if (!j || selected.value !== j.path || !codeVisible.value || previewPath.value !== j.path) return;
  pendingJump.value = null;
  const cv = codeViewer.value;
  if (!cv) return;
  // 文件内跑同一查询,优先精确定位到命中行上的匹配;行上无匹配退化到行首
  const ranges = cv.runFind(j.query);
  const idx = ranges.findIndex((r) => r.line === j.line);
  if (idx >= 0) {
    findTotal.value = ranges.length;
    findIndex.value = cv.gotoMatch(idx);
  } else {
    cv.revealLine(j.line);
  }
}

watch([selected, previewText, codeVisible], () => void nextTick(tryJump), { flush: "post" });

// ── 快捷键:Ctrl+F 文件内查找 / Ctrl+Shift+F 全文搜索 / F3 与 Esc ────────────
function onKeydown(e: KeyboardEvent) {
  const key = e.key.toLowerCase();
  if (e.ctrlKey && key === "f") {
    if (e.shiftKey) {
      e.preventDefault();
      leftView.value = "search";
      searchPanelRef.value?.focusInput();
    } else if (codeVisible.value) {
      e.preventDefault();
      openFind();
    }
  } else if (e.key === "F3" || (e.ctrlKey && key === "g")) {
    if (findOpen.value) {
      e.preventDefault();
      findStep(e.shiftKey ? -1 : 1);
    }
  } else if (e.key === "Escape" && findOpen.value) {
    closeFind();
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));

// ── Markdown 渲染(复用 README 抽屉的渲染器与控件配置) ────────────────────────
// 相对路径图片/链接的解析基准 = 文件所在目录
const mdBasePath = computed(() =>
  project.value && selected.value
    ? resolvePath(rootPath.value, selected.value.split("/").slice(0, -1).join("/") || ".")
    : rootPath.value,
);
provide(MD_BASE_PATH_KEY, () => mdBasePath.value);

const nodeRenderers: NodeRenderers = {
  image: MdImage,
  link: MdLink,
};

const controls: ControlsConfig = {
  table: {
    copy: true,
    download: true,
    fullscreen: true,
    customize: createTableCustomize(t),
  },
  code: { copy: true, collapse: true },
};

const beforeDownload = createBeforeDownload(t);

// 见 ReadmeDrawer:阻止库把 shadcn 变量内联成非法色值,MD 主题交给 CSS 层
const detachedThemeEl = document.createElement("div");
const themeElement = () => detachedThemeEl;

/** 渲染态链接点击:外链交给系统浏览器,相对路径用系统默认程序打开 */
async function onBodyClick(e: MouseEvent) {
  const a = (e.target as HTMLElement).closest("a");
  if (!a) return;
  const href = a.getAttribute("href");
  e.preventDefault();
  if (!href || href.startsWith("#")) return;
  try {
    if (hasScheme(href)) {
      await openUrl(href);
    } else {
      await openPath(resolvePath(mdBasePath.value, href));
    }
  } catch {
    // 目标不存在等情况静默忽略
  }
}

// ── 左栏宽度拖拽 ────────────────────────────────────────────────────────────
const treeWidth = useLocalStorage("repomeow:files-tree-w", 280);
const TREE_MIN_W = 180;
const TREE_MAX_W = 560;

function startTreeResize(e: PointerEvent) {
  e.preventDefault();
  const startX = e.clientX;
  const startW = treeWidth.value;
  const onMove = (ev: PointerEvent) => {
    treeWidth.value = Math.min(TREE_MAX_W, Math.max(TREE_MIN_W, startW + ev.clientX - startX));
  };
  const onUp = () => {
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
  };
  window.addEventListener("pointermove", onMove);
  window.addEventListener("pointerup", onUp);
}
</script>

<template>
  <div v-if="project" class="flex h-full flex-col">
    <header class="relative flex shrink-0 items-center gap-2 border-b px-4 py-3">
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8 shrink-0"
        :title="t('files.back')"
        @click="router.push(`/projects/${project.id}`)"
      >
        <ArrowLeft class="h-4 w-4" />
      </Button>
      <FolderTree class="h-4 w-4 shrink-0 text-muted-foreground" />
      <span class="min-w-0 flex-1 truncate text-sm font-medium" :title="rootPath">
        {{ project.name }}
      </span>
      <!-- 文件搜索:右侧按钮触发,顶部中间搜索框 + 下拉选项 -->
      <div
        v-if="fileSearchOpen"
        ref="fileSearchBox"
        class="absolute left-1/2 top-1/2 z-50 w-[min(28rem,60%)] -translate-x-1/2 -translate-y-1/2"
      >
        <Search
          class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="fileSearchText"
          :placeholder="t('files.searchPlaceholder')"
          class="h-8 bg-background pl-8 text-sm"
          @keydown="onFileSearchKeydown"
        />
        <div
          v-if="fileSearchText.trim()"
          class="absolute left-0 right-0 top-full mt-1 max-h-80 overflow-auto rounded-md border bg-popover p-1 shadow-md"
        >
          <p
            v-if="!fileSearchResults.length"
            class="px-2 py-3 text-center text-xs text-muted-foreground"
          >
            {{ t("files.noMatch") }}
          </p>
          <template v-else>
            <button
              v-for="(f, i) in fileSearchResults"
              :key="f.path"
              type="button"
              class="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm"
              :class="i === fileSearchIndex ? 'bg-accent text-accent-foreground' : ''"
              :title="f.path"
              @mouseenter="fileSearchIndex = i"
              @click="openFileFromSearch(f.path)"
            >
              <Icon
                :icon="fileIcon(f.path.slice(f.path.lastIndexOf('/') + 1))"
                class="h-4 w-4 shrink-0"
              />
              <span class="min-w-0 truncate">{{ f.path }}</span>
            </button>
            <p
              v-if="fileSearchLimited"
              class="border-t px-2 py-1.5 text-center text-xs text-muted-foreground"
            >
              {{ t("files.searchLimited", { count: FILE_SEARCH_LIMIT }) }}
            </p>
          </template>
        </div>
      </div>
      <Button
        ref="fileSearchBtn"
        variant="ghost"
        size="icon"
        class="h-8 w-8 shrink-0"
        :class="fileSearchOpen ? 'bg-accent' : ''"
        :title="t('files.searchPlaceholder')"
        @click="toggleFileSearch"
      >
        <Search class="h-4 w-4" />
      </Button>
    </header>

    <div class="flex min-h-0 flex-1">
      <!-- 左侧:文件树 / 全文搜索双视图 -->
      <div
        class="flex h-full min-h-0 shrink-0 flex-col border-r"
        :style="{ width: `${treeWidth}px` }"
      >
        <div class="flex shrink-0 items-center gap-1.5 border-b px-2 py-2">
          <span class="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
            {{ leftView === "tree" ? t("files.treeView") : t("files.textSearchTitle") }}
          </span>
          <div class="flex shrink-0 items-center gap-0.5">
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="leftView === 'tree' ? 'bg-accent' : ''"
              :title="t('files.treeView')"
              @click="leftView = 'tree'"
            >
              <FolderTree class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="leftView === 'search' ? 'bg-accent' : ''"
              :title="t('files.searchView')"
              @click="leftView = 'search'"
            >
              <Search class="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
        <template v-if="leftView === 'tree'">
          <ScrollArea class="min-h-0 flex-1">
            <p v-if="listLoading" class="p-4 text-sm text-muted-foreground">
              {{ t("common.loading") }}
            </p>
            <p v-else-if="listError" class="p-4 text-sm text-destructive">
              {{ t("files.listFailed") }}
            </p>
            <p v-else-if="rootEmpty" class="p-4 text-sm text-muted-foreground">
              {{ t("files.empty") }}
            </p>
            <FileTreeList
              v-else
              :rows="visibleRows"
              :selected="selected"
              @select="(row) => (selected = row.fullPath)"
              @toggle="(row) => toggleFolder(row.fullPath)"
            />
          </ScrollArea>
        </template>
        <TextSearchPanel v-else ref="searchPanelRef" :root="rootPath" @open="onSearchOpen" />
      </div>

      <!-- 拖拽条 -->
      <div
        class="w-1.5 shrink-0 cursor-col-resize transition-colors hover:bg-primary/50"
        @pointerdown="startTreeResize"
      />

      <!-- 右侧预览 -->
      <div class="flex h-full min-h-0 min-w-0 flex-1 flex-col">
        <div class="flex shrink-0 items-center gap-2 border-b px-3 py-2">
          <span
            class="min-w-0 flex-1 truncate text-sm text-muted-foreground"
            :title="selected ?? undefined"
          >
            {{ selected ?? t("files.selectHint") }}
          </span>
          <div v-if="isMarkdown && previewText !== null" class="flex shrink-0 items-center gap-1">
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="mdMode === 'rendered' ? 'bg-accent' : ''"
              :title="t('files.rendered')"
              @click="mdMode = 'rendered'"
            >
              <Eye class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="mdMode === 'source' ? 'bg-accent' : ''"
              :title="t('files.source')"
              @click="mdMode = 'source'"
            >
              <Code class="h-3.5 w-3.5" />
            </Button>
          </div>
          <div v-if="isSvg" class="flex shrink-0 items-center gap-1">
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="svgMode === 'preview' ? 'bg-accent' : ''"
              :title="t('files.rendered')"
              @click="svgMode = 'preview'"
            >
              <Eye class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :class="svgMode === 'source' ? 'bg-accent' : ''"
              :title="t('files.source')"
              @click="svgMode = 'source'"
            >
              <Code class="h-3.5 w-3.5" />
            </Button>
          </div>
          <Button
            v-if="codeVisible"
            variant="ghost"
            size="icon"
            class="h-7 w-7 shrink-0"
            :class="codeWrap ? 'bg-accent' : ''"
            :title="t('files.wrap')"
            @click="codeWrap = !codeWrap"
          >
            <WrapText class="h-3.5 w-3.5" />
          </Button>
        </div>

        <div
          v-if="previewTruncated"
          class="shrink-0 border-b bg-muted/50 px-3 py-1.5 text-xs text-muted-foreground"
        >
          {{ t("files.truncated") }}
        </div>

        <!-- 文件内查找条(Ctrl+F) -->
        <FindBar
          v-if="findOpen && codeVisible"
          ref="findBarRef"
          v-model:text="findText"
          :modes="{ caseSensitive: findCase, wholeWord: findWord, useRegex: findRegex }"
          :total="findTotal"
          :current="findIndex"
          :invalid="findInvalid"
          @toggle="onFindToggle"
          @next="findStep(1)"
          @prev="findStep(-1)"
          @close="closeFind"
        />

        <!-- 非代码分支(空态/错误/二进制/图片/MD 渲染):原生双向滚动容器 -->
        <div v-if="!codeVisible" class="min-h-0 flex-1 overflow-auto">
          <div
            v-if="!selected"
            class="flex h-full flex-col items-center justify-center gap-2 p-10 text-muted-foreground"
          >
            <FileQuestion class="h-8 w-8" />
            <p class="text-sm">{{ t("files.selectHint") }}</p>
          </div>
          <p v-else-if="previewError" class="p-6 text-sm text-destructive">
            {{ t("files.loadFailed") }}
          </p>
          <p v-else-if="previewBinary" class="p-6 text-sm text-muted-foreground">
            {{ t("files.binary") }}
          </p>
          <ImageViewer
            v-else-if="isImage && !svgSource"
            :src="imageSrc"
            :svg="isSvg"
            :alt="selected ?? undefined"
          />
          <!-- md 内容未就位(残留文本来自非 md 文件)时短暂空白,防止上一个文件的文本被当 Markdown 渲染闪现 -->
          <div v-else-if="!mdRenderable" />
          <div v-else class="p-6 text-sm" @click="onBodyClick">
            <Markdown
              mode="static"
              :content="previewText ?? ''"
              :controls="controls"
              :node-renderers="nodeRenderers"
              :theme-element="themeElement"
              :locale="settingsStore.language"
              :before-download="beforeDownload"
            />
          </div>
        </div>
        <!-- 代码视图:CodeMirror 只读,行号/折叠/换行/滚动均由其自带能力承担;path 传 previewPath
             保证语言高亮与文本恒为同一文件(保留旧内容期间不会旧文本配新扩展名高亮) -->
        <CodeViewer
          v-else
          ref="codeViewer"
          :text="previewText ?? ''"
          :path="previewPath ?? ''"
          :wrap="codeWrap"
        />
      </div>
    </div>
  </div>

  <div
    v-else
    class="flex h-full flex-col items-center justify-center gap-3 text-sm text-muted-foreground"
  >
    <p>{{ t("projects.detail.notFound") }}</p>
    <Button variant="outline" size="sm" @click="router.push('/')">
      {{ t("projects.detail.backToListShort") }}
    </Button>
  </div>
</template>
