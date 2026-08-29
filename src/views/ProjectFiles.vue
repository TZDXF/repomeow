<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, provide, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "vue-sonner";
import { ArrowLeft, Code, ExternalLink, Eye, FileQuestion, FolderTree, WrapText } from "@lucide/vue";
import { useLocalStorage } from "@vueuse/core";
import { Markdown, type ControlsConfig, type NodeRenderers } from "vue-stream-markdown";
import { Button } from "@/components/ui/button";
import CodeViewer from "@/components/files/CodeViewer.vue";
import FileNameSearch from "@/components/files/FileNameSearch.vue";
import FindBar from "@/components/files/FindBar.vue";
import ProjectFilesSidebar from "@/components/files/ProjectFilesSidebar.vue";
import MdImage from "@/components/markdown/MdImage.vue";
import MdLink from "@/components/markdown/MdLink.vue";
import { MD_BASE_PATH_KEY } from "@/components/markdown/keys";
import ImageViewer from "@/components/files/ImageViewer.vue";
import { cmd } from "@/lib/tauri";
import { extOf, IMAGE_EXTS } from "@/lib/file-kind";
import { hasScheme, resolvePath } from "@/lib/markdown";
import { openPathWith, sortOpenWithOptions } from "@/lib/open-with";
import { createBeforeDownload, createTableCustomize } from "@/lib/markdown-download";
import type { FindQuery } from "@/lib/text-search";
import { useFileFind } from "@/composables/files/useFileFind";
import { useLazyProjectFiles } from "@/composables/files/useLazyProjectFiles";
import { useSettingsStore } from "@/stores/settings";
import { useProjectsStore } from "@/stores/projects";
import type { FilePreview, Project } from "@/types";

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

// ── 选中与预览 ──────────────────────────────────────────────────────────────
const selected = ref<string | null>(null);
const { listError, listLoading, revealPath, rootEmpty, toggleFolder, visibleRows } =
  useLazyProjectFiles({ rootPath, selected });

async function openFileFromSearch(path: string) {
  selected.value = path;
  leftView.value = "tree";
  await revealPath(path);
}

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

// ── 在编辑器中打开当前文件(用设置里的默认打开方式,与提交详情面板同一逻辑) ──
async function openSelectedInIde() {
  const path = selected.value;
  if (!path) return;
  const option = sortOpenWithOptions(
    settingsStore.openWithOrder,
    settingsStore.customOpenWith,
  ).find((candidate) => candidate.id === settingsStore.defaultOpenWith);
  if (!option) return;
  try {
    await openPathWith(option, resolvePath(rootPath.value, path));
  } catch (e) {
    toast.error(String(e));
  }
}

/** Markdown 渲染分支可否使用当前内容:残留内容须本身来自 md 文件——
 *  代码文件的旧文本以 MD 渲染会闪现错内容,等待新内容期间显示空白(md→md 仍保留旧文防闪) */
const mdRenderable = computed(
  () => previewPath.value !== null && MD_EXTS.has(extOf(previewPath.value)),
);

// ── 搜索:左栏全文搜索面板 + 文件内查找条 ─────────────────────────────────────
const codeViewer = ref<InstanceType<typeof CodeViewer> | null>(null);
const findBarRef = ref<InstanceType<typeof FindBar> | null>(null);
const sidebarRef = ref<InstanceType<typeof ProjectFilesSidebar> | null>(null);

const leftView = ref<"tree" | "search">("tree");
const {
  closeFind,
  findCase,
  findIndex,
  findInvalid,
  findOpen,
  findRegex,
  findStep,
  findText,
  findTotal,
  findWord,
  onFindToggle,
  openFind,
} = useFileFind({ codeViewer, codeVisible, findBarRef, previewText });

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
      sidebarRef.value?.focusSearch();
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

// 阻止库把 shadcn 变量内联成非法色值,MD 主题交给 CSS 层
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
      <FileNameSearch :root="rootPath" @open="openFileFromSearch" />
    </header>

    <div class="flex min-h-0 flex-1">
      <!-- 左侧:文件树 / 全文搜索双视图 -->
      <div
        class="flex h-full min-h-0 shrink-0 flex-col border-r"
        :style="{ width: `${treeWidth}px` }"
      >
        <ProjectFilesSidebar
          ref="sidebarRef"
          v-model:view="leftView"
          :empty="rootEmpty"
          :error="listError"
          :loading="listLoading"
          :root="rootPath"
          :rows="visibleRows"
          :selected="selected"
          @open="onSearchOpen"
          @select="(path) => (selected = path)"
          @toggle="toggleFolder"
        />
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
          <Button
            v-if="selected"
            variant="ghost"
            size="icon"
            class="h-7 w-7 shrink-0"
            :title="t('files.openInIde')"
            @click="openSelectedInIde"
          >
            <ExternalLink class="h-3.5 w-3.5" />
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
