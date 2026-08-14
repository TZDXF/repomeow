<script setup lang="ts">
import { computed, provide, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  ChevronDown,
  ChevronRight,
  Code,
  Eye,
  FileQuestion,
  FolderTree,
  Search,
  WrapText,
} from "@lucide/vue";
import { useLocalStorage } from "@vueuse/core";
import { Markdown, type ControlsConfig, type NodeRenderers } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import CodeViewer from "@/components/files/CodeViewer.vue";
import MdImage from "@/components/markdown/MdImage.vue";
import MdLink from "@/components/markdown/MdLink.vue";
import { MD_BASE_PATH_KEY } from "@/components/markdown/keys";
import ImageViewer from "@/components/files/ImageViewer.vue";
import { cmd } from "@/lib/tauri";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import { buildFileTree, type FileTreeNode } from "@/lib/file-tree";
import { fileIcon, folderIcon } from "@/lib/file-icons";
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

// ── 文件清单 ────────────────────────────────────────────────────────────────
const files = ref<ProjectFileEntry[]>([]);
const listLoading = ref(false);
const listError = ref(false);
const search = ref("");

const filtered = computed(() => {
  const q = search.value.trim().toLowerCase();
  if (!q) return files.value;
  return files.value.filter((f) => f.path.toLowerCase().includes(q));
});

const tree = computed(() => buildFileTree(filtered.value));

// 目录是否整体被 git 排除:其下所有文件均 ignored 时目录同样灰显
const ignoredDirs = computed(() => {
  const total = new Map<string, number>();
  const ignored = new Map<string, number>();
  for (const f of files.value) {
    const segs = f.path.split("/");
    let prefix = "";
    for (let i = 0; i < segs.length - 1; i++) {
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      total.set(prefix, (total.get(prefix) ?? 0) + 1);
      if (f.ignored) ignored.set(prefix, (ignored.get(prefix) ?? 0) + 1);
    }
  }
  const result = new Set<string>();
  for (const [dir, count] of total) {
    if (ignored.get(dir) === count) result.add(dir);
  }
  return result;
});

// 折叠的目录集合:默认全部折叠,避免全量文件树(含 node_modules)首次渲染数万行
const collapsedFolders = ref(new Set<string>());

function collectCollapsed(nodes: FileTreeNode<ProjectFileEntry>[], out: Set<string>) {
  for (const node of nodes) {
    if (node.file !== null) continue;
    out.add(node.fullPath);
    collectCollapsed(node.children, out);
  }
}

function toggleFolder(fullPath: string) {
  const next = new Set(collapsedFolders.value);
  if (next.has(fullPath)) next.delete(fullPath);
  else next.add(fullPath);
  collapsedFolders.value = next;
}

interface FileRow {
  key: string;
  name: string;
  fullPath: string;
  isDir: boolean;
  depth: number;
  /** iconify 图标名(vscode-icons 集) */
  icon: string;
  /** 被 git 排除(.gitignore/.ignore),降低灰度显示 */
  dimmed: boolean;
}

const visibleRows = computed<FileRow[]>(() => {
  // 搜索时展示扁平匹配清单,不看折叠状态
  if (search.value.trim()) {
    return filtered.value.map((f) => ({
      key: f.path,
      name: f.path,
      fullPath: f.path,
      isDir: false,
      depth: 0,
      icon: fileIcon(f.path.slice(f.path.lastIndexOf("/") + 1)),
      dimmed: f.ignored,
    }));
  }
  const out: FileRow[] = [];
  const walk = (nodes: FileTreeNode<ProjectFileEntry>[], depth: number) => {
    for (const node of nodes) {
      const isDir = node.file === null;
      const open = isDir && !collapsedFolders.value.has(node.fullPath);
      out.push({
        key: node.fullPath,
        name: node.name,
        fullPath: node.fullPath,
        isDir,
        depth,
        icon: isDir ? folderIcon(node.name, open) : fileIcon(node.name),
        dimmed: isDir ? ignoredDirs.value.has(node.fullPath) : (node.file?.ignored ?? false),
      });
      if (open) {
        walk(node.children, depth + 1);
      }
    }
  };
  walk(tree.value, 0);
  return out;
});

async function loadFiles() {
  if (!project.value) return;
  listLoading.value = true;
  listError.value = false;
  try {
    files.value = await cmd<ProjectFileEntry[]>("list_project_files", { path: project.value.path });
    const collapsed = new Set<string>();
    collectCollapsed(buildFileTree(files.value), collapsed);
    collapsedFolders.value = collapsed;
    // 刷新后选中文件已不存在时清掉选中
    if (selected.value && !files.value.some((f) => f.path === selected.value)) {
      selected.value = null;
    }
  } catch {
    files.value = [];
    listError.value = true;
  } finally {
    listLoading.value = false;
  }
}

watch(() => project.value?.id, loadFiles, { immediate: true });

// ── 选中与预览 ──────────────────────────────────────────────────────────────
const selected = ref<string | null>(null);
const previewError = ref(false);
const previewText = ref<string | null>(null);
const previewTruncated = ref(false);
const previewBinary = ref(false);
let previewSeq = 0;

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "avif"]);
const MD_EXTS = new Set(["md", "markdown"]);

function extOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

const selectedExt = computed(() => (selected.value ? extOf(selected.value) : ""));
const isImage = computed(() => IMAGE_EXTS.has(selectedExt.value));
const isMarkdown = computed(() => MD_EXTS.has(selectedExt.value));
const isSvg = computed(() => selectedExt.value === "svg");

// svg 兼具图像与文本两种形态:源码模式下按文本读取走代码视图,其余图片 asset 直显
const svgMode = ref<"preview" | "source">("preview");
const svgSource = computed(() => isSvg.value && svgMode.value === "source");

const imageSrc = computed(() =>
  selected.value && isImage.value && project.value
    ? convertFileSrc(resolvePath(project.value.path, selected.value))
    : "",
);

function onRowClick(row: FileRow) {
  if (row.isDir) toggleFolder(row.fullPath);
  else selected.value = row.fullPath;
}

// 切换文件不显示 loading:本地读取很快,loading 只会闪烁;
// 保留旧内容直到新内容就位(序号防串台),仅错误/二进制等状态标记随切换即清
watch([selected, svgSource], async ([path]) => {
  const mySeq = ++previewSeq;
  previewError.value = false;
  previewBinary.value = false;
  previewTruncated.value = false;
  if (!path || !project.value) {
    previewText.value = null;
    return;
  }
  if (IMAGE_EXTS.has(extOf(path)) && !svgSource.value) {
    previewText.value = null; // 图片走 asset 协议直显,不读内容(svg 源码模式除外)
    return;
  }
  try {
    const res = await cmd<FilePreview>("read_file_preview", {
      root: project.value.path,
      relPath: path,
    });
    if (mySeq !== previewSeq) return;
    if (res.text === null) {
      previewText.value = null;
      previewBinary.value = true;
    } else {
      previewText.value = res.text;
      previewTruncated.value = res.truncated;
    }
  } catch {
    if (mySeq === previewSeq) {
      previewText.value = null;
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

// ── Markdown 渲染(复用 README 抽屉的渲染器与控件配置) ────────────────────────
// 相对路径图片/链接的解析基准 = 文件所在目录
const mdBasePath = computed(() =>
  project.value && selected.value
    ? resolvePath(project.value.path, selected.value.split("/").slice(0, -1).join("/") || ".")
    : (project.value?.path ?? ""),
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
    <header class="flex shrink-0 items-center gap-2 border-b px-4 py-3">
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
      <span class="min-w-0 truncate text-sm font-medium" :title="project.path">
        {{ project.name }}
      </span>
    </header>

    <div class="flex min-h-0 flex-1">
      <!-- 左侧文件树 -->
      <div
        class="flex h-full min-h-0 shrink-0 flex-col border-r"
        :style="{ width: `${treeWidth}px` }"
      >
        <div class="shrink-0 border-b p-2">
          <div class="relative">
            <Search
              class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              v-model="search"
              :placeholder="t('files.searchPlaceholder')"
              class="h-8 pl-8 text-sm"
            />
          </div>
        </div>
        <ScrollArea class="min-h-0 flex-1">
          <p v-if="listLoading" class="p-4 text-sm text-muted-foreground">
            {{ t("common.loading") }}
          </p>
          <p v-else-if="listError" class="p-4 text-sm text-destructive">
            {{ t("files.listFailed") }}
          </p>
          <p v-else-if="!files.length" class="p-4 text-sm text-muted-foreground">
            {{ t("files.empty") }}
          </p>
          <p v-else-if="!visibleRows.length" class="p-4 text-sm text-muted-foreground">
            {{ t("files.noMatch") }}
          </p>
          <div v-else class="py-1">
            <button
              v-for="row in visibleRows"
              :key="row.key"
              class="flex w-full items-center gap-1 py-1 pr-2 text-left text-sm hover:bg-accent"
              :class="[
                selected === row.fullPath ? 'bg-accent text-accent-foreground' : '',
                row.dimmed ? 'opacity-50' : '',
              ]"
              :style="{ paddingLeft: `${row.depth * 14 + 8}px` }"
              :title="row.fullPath"
              @click="onRowClick(row)"
            >
              <template v-if="row.isDir">
                <ChevronDown
                  v-if="!collapsedFolders.has(row.fullPath)"
                  class="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                />
                <ChevronRight v-else class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              </template>
              <span v-else class="w-3.5 shrink-0" />
              <Icon :icon="row.icon" class="h-4 w-4 shrink-0" />
              <span class="min-w-0 truncate">{{ row.name }}</span>
            </button>
          </div>
        </ScrollArea>
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
        <!-- 代码视图:CodeMirror 只读,行号/折叠/换行/滚动均由其自带能力承担 -->
        <CodeViewer v-else :text="previewText ?? ''" :path="selected ?? ''" :wrap="codeWrap" />
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
