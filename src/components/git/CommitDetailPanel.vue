<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useElementSize, useLocalStorage } from "@vueuse/core";
import {
  Columns2,
  Copy,
  FolderTree,
  GitBranch,
  List,
  Loader2,
  PanelRightClose,
  Rows2,
  Tag as TagIcon,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import FileTreeList from "@/components/common/FileTreeList.vue";
import DiffViewer from "@/components/git/DiffViewer.vue";
import ImageDiffPreview from "@/components/git/ImageDiffPreview.vue";
import SemanticChangeList from "@/components/git/SemanticChangeList.vue";
import SemanticImpactPanel from "@/components/semantic/SemanticImpactPanel.vue";
import { statusClass } from "@/lib/git-status";
import { buildFileTree, flatFileRows, flattenVisibleTree } from "@/lib/file-tree";
import { extOf, imageMimeOf, isImagePath } from "@/lib/file-kind";
import { openPathWith, sortOpenWithOptions } from "@/lib/open-with";
import { baseName } from "@/lib/path";
import { semanticTotal } from "@/lib/semantic";
import { copyToClipboard } from "@/lib/utils";
import { cmd } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settings";
import type {
  GitCommitFile,
  GitCommitFileDiff,
  GitGraphCommit,
  SemanticChange,
  SemanticDiffResult,
  SemanticEntityRef,
} from "@/types";

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

// --- sem 实体级差异:仅切到“实体”列表时懒加载；结果按 commit hash 防止串位 ---
const listMode = ref<"files" | "semantic">("files");
const semanticResult = ref<SemanticDiffResult | null>(null);
const semanticLoading = ref(false);
const semanticError = ref("");
const semanticLoadedForHash = ref("");

// --- 单文件 diff:本组件只负责取数,解析/着色/折叠/并排渲染全部在 DiffViewer ---
const selectedPath = ref<string | null>(null);
const diff = ref<GitCommitFileDiff | null>(null);
const diffLoading = ref(false);
const diffError = ref("");

// --- 图片 diff:二进制图片无文本 diff 可渲染,新旧版本 blob 转 data URL 由
//     ImageDiffPreview 直接预览(与 ProjectFiles 的图片查看同款交互) ---
interface DiffImagePane {
  key: string;
  label: string;
  path: string;
  src: string;
  svg: boolean;
}
const imagePanes = ref<DiffImagePane[]>([]);
const imageLoading = ref(false);
const imageError = ref("");

const selectedIsImage = computed(() =>
  selectedPath.value ? isImagePath(selectedPath.value) : false,
);

/** 忽略空白差异模式(持久化):none 不忽略 / eol 行尾 / change 空白数量变化 / all 全部空白;
 *  与 DiffViewer 的 v-model 同源,模式变化时按新模式重取当前文件 diff(行集会变) */
const ignoreWs = useLocalStorage<"none" | "eol" | "change" | "all">(
  "repomeow:commit-diff-ignore-ws",
  "none",
);

/** 合并提交(多父)diff-tree 无输出,展示提示而非空列表 */
const isMerge = computed(() => props.commit.parents.length > 1);

async function loadFiles() {
  filesLoading.value = true;
  filesError.value = "";
  files.value = [];
  selectedPath.value = null;
  diff.value = null;
  diffError.value = "";
  imagePanes.value = [];
  imageError.value = "";
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

async function loadSemantic(force = false) {
  const hash = props.commit.hash;
  if (isMerge.value || semanticLoading.value) {
    return;
  }
  if (!force && semanticLoadedForHash.value === hash && semanticResult.value) {
    return;
  }
  semanticLoading.value = true;
  semanticError.value = "";
  try {
    const result = await cmd<SemanticDiffResult>("semantic_commit_diff", {
      path: props.projectPath,
      hash,
    });
    if (props.commit.hash !== hash) {
      return;
    }
    semanticResult.value = result;
    semanticLoadedForHash.value = hash;
  } catch (e) {
    if (props.commit.hash !== hash) {
      return;
    }
    semanticError.value = String(e);
  } finally {
    if (props.commit.hash === hash) {
      semanticLoading.value = false;
    }
  }
}

function setListMode(mode: "files" | "semantic") {
  listMode.value = mode;
  if (mode === "semantic") {
    void loadSemantic();
  }
}

watch(
  () => props.commit.hash,
  () => {
    semanticResult.value = null;
    semanticError.value = "";
    semanticLoadedForHash.value = "";
    semanticLoading.value = false;
    if (isMerge.value) {
      listMode.value = "files";
    }
    void loadFiles();
    if (listMode.value === "semantic") {
      void loadSemantic();
    }
  },
  { immediate: true },
);

// --- 影响分析(实体视图入口):sem impact 基于当前工作树/HEAD,非 HEAD 提交需提示 ---
const impactOpen = ref(false);
const impactEntity = ref<SemanticEntityRef | null>(null);

function openImpact(change: SemanticChange) {
  impactEntity.value = {
    entityId: change.entityId || null,
    name: change.entityName,
    entityType: change.entityType,
    filePath: change.filePath,
    startLine: change.startLine,
    endLine: change.endLine,
  };
  impactOpen.value = true;
}

/** 影响分析跳到源码:GitGraph 上下文用默认编辑器打开对应文件 */
async function openImpactTarget(entity: SemanticEntityRef) {
  const option = sortOpenWithOptions(settings.openWithOrder, settings.customOpenWith).find(
    (candidate) => candidate.id === settings.defaultOpenWith,
  );
  if (!option) return;
  try {
    await openPathWith(option, `${props.projectPath}/${entity.filePath}`);
  } catch (e) {
    toast.error(String(e));
  }
}

/** 语义变更点击的行定位请求:seq 递增保证同一行重复点击也触发 */
const diffReveal = ref<{ line: number; seq: number } | null>(null);
let diffRevealSeq = 0;

function selectSemanticFile(filePath: string, oldFilePath: string | null, line: number | null) {
  const file = files.value.find(
    (candidate) =>
      candidate.path === filePath ||
      candidate.old_path === filePath ||
      (oldFilePath !== null &&
        (candidate.path === oldFilePath || candidate.old_path === oldFilePath)),
  );
  if (file) {
    void selectFile(file, false, line);
  }
}

async function selectFile(file: GitCommitFile, force = false, revealLine: number | null = null) {
  // 定位请求先于早退判断下发:同文件重复点击时 diff 不重取,但 DiffViewer 仍按新 seq 滚动
  diffReveal.value =
    revealLine != null && revealLine > 0 ? { line: revealLine, seq: ++diffRevealSeq } : null;
  if (!force && selectedPath.value === file.path && (diff.value || imagePanes.value.length)) {
    return;
  }
  selectedPath.value = file.path;
  // 图片:不走文本 diff,取新旧版本 blob 预览(忽略空白模式对二进制无意义,不参与其重取)
  if (isImagePath(file.path)) {
    diff.value = null;
    diffError.value = "";
    diffLoading.value = false;
    void loadImageDiff(file);
    return;
  }
  imagePanes.value = [];
  // 切文件不清空旧 diff、不转圈:旧内容保留到新结果落地后由 DiffViewer 同帧替换;
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
    // 快速连点 A→B:旧响应可能晚于新选择返回,不做 stale 校验会把 A 的 diff 写到 B 的标题下
    if (selectedPath.value !== file.path) return;
    diff.value = result;
  } catch (e) {
    if (selectedPath.value !== file.path) return;
    diffError.value = String(e);
  } finally {
    if (selectedPath.value === file.path) diffLoading.value = false;
  }
}

// 图片 diff 取数:新增文件无旧版本、删除文件无新版本,其余(含重命名/类型变更)两侧都取;
// blob 在某一版本不存在时该侧直接不渲染。旧版本从父提交的树读,重命名走 old_path
async function loadImageDiff(file: GitCommitFile) {
  const wantOld = file.status !== "A";
  const wantNew = file.status !== "D";
  imageLoading.value = true;
  imageError.value = "";
  try {
    const [oldB64, newB64] = await Promise.all([
      wantOld
        ? cmd<string | null>("git_commit_file_blob", {
            path: props.projectPath,
            hash: props.commit.hash,
            filePath: file.old_path ?? file.path,
            parent: true,
          })
        : Promise.resolve(null),
      wantNew
        ? cmd<string | null>("git_commit_file_blob", {
            path: props.projectPath,
            hash: props.commit.hash,
            filePath: file.path,
            parent: false,
          })
        : Promise.resolve(null),
    ]);
    if (selectedPath.value !== file.path) return;
    const panes: DiffImagePane[] = [];
    if (oldB64 !== null) {
      const p = file.old_path ?? file.path;
      panes.push({
        key: "old",
        label: t("git.graph.detail.imageOld"),
        path: p,
        src: `data:${imageMimeOf(p)};base64,${oldB64}`,
        svg: extOf(p) === "svg",
      });
    }
    if (newB64 !== null) {
      panes.push({
        key: "new",
        label: t("git.graph.detail.imageNew"),
        path: file.path,
        src: `data:${imageMimeOf(file.path)};base64,${newB64}`,
        svg: extOf(file.path) === "svg",
      });
    }
    imagePanes.value = panes;
  } catch (e) {
    if (selectedPath.value !== file.path) return;
    imagePanes.value = [];
    imageError.value = String(e);
  } finally {
    if (selectedPath.value === file.path) imageLoading.value = false;
  }
}

// 忽略空白模式变化:按新模式重取当前文件 diff(图片是二进制,不受空白模式影响,跳过)
watch(ignoreWs, () => {
  const file = selectedFile.value;
  if (file && !isImagePath(file.path)) void selectFile(file, true);
});

// --- 树形展示:按目录层级聚合,折叠状态记忆;行化(树形/平铺)走 file-tree 统一辅助 ---
const fileTree = computed(() => buildFileTree(files.value));

const treeRows = computed(() => flattenVisibleTree(fileTree.value, collapsedFolders.value));

/** 平铺行:只显示文件名,完整路径放 title;baseName 走 @/lib/path */
const flatRows = computed(() =>
  flatFileRows(files.value, {
    name: (f) => (f.old_path ? `${baseName(f.old_path)} → ${baseName(f.path)}` : baseName(f.path)),
    title: (f) => (f.old_path ? `${f.old_path} → ${f.path}` : f.path),
  }),
);

function toggleFolder(fullPath: string) {
  const next = new Set(collapsedFolders.value);
  if (next.has(fullPath)) {
    next.delete(fullPath);
  } else {
    next.add(fullPath);
  }
  collapsedFolders.value = next;
}

/** 当前选中文件(已删除的文件不提供 IDE 打开:工作区已不存在) */
const selectedFile = computed(() => files.value.find((f) => f.path === selectedPath.value) ?? null);
const canOpenInIde = computed(() => !!selectedFile.value && selectedFile.value.status !== "D");

/** 并排是否适用:新增/删除文件一侧必然全空,强制逐行视图并隐藏切换按钮 */
const splitApplicable = computed(
  () => selectedFile.value?.status !== "A" && selectedFile.value?.status !== "D",
);

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
  await copyToClipboard(hash);
}

// --- 布局:vertical = 列表在上 diff 在下;horizontal = 列表与 diff 左右分列(diff 单独一列) ---
/** 布局切换(持久化) */
const layout = useLocalStorage<"vertical" | "horizontal">(
  "repomeow:commit-detail-layout",
  "vertical",
);

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
  // 拖拽中组件被卸载:残留 pointermove/pointerup 监听器仍会引用已卸载组件的状态,显式移除避免泄漏
  for (const fn of listResizeCleanups) fn();
  listResizeCleanups = [];
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
          <div class="flex items-center rounded-md bg-muted p-0.5 text-[10px]">
            <button
              type="button"
              class="rounded px-2 py-0.5 transition-colors"
              :class="
                listMode === 'files'
                  ? 'bg-background text-foreground shadow-sm'
                  : 'text-muted-foreground'
              "
              @click="setListMode('files')"
            >
              {{ t("git.graph.detail.filesCount", { count: files.length }) }}
            </button>
            <button
              v-if="!isMerge"
              type="button"
              class="rounded px-2 py-0.5 transition-colors"
              :class="
                listMode === 'semantic'
                  ? 'bg-background text-foreground shadow-sm'
                  : 'text-muted-foreground'
              "
              @click="setListMode('semantic')"
            >
              {{ t("git.graph.semantic.entities", { count: semanticTotal(semanticResult) }) }}
            </button>
          </div>
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
              v-if="listMode === 'files'"
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
          <SemanticChangeList
            v-if="listMode === 'semantic'"
            :result="semanticResult"
            :loading="semanticLoading"
            :error="semanticError"
            :selected-path="selectedPath"
            @select="selectSemanticFile"
            @retry="loadSemantic(true)"
            @impact="openImpact"
          />
          <template v-else>
            <div v-if="filesLoading" class="flex h-full items-center justify-center">
              <Loader2 class="h-4 w-4 animate-spin text-muted-foreground" />
            </div>
            <p v-else-if="filesError" class="px-3 py-2 text-xs text-destructive">
              {{ t("git.graph.detail.filesLoadFailed") }}:{{ filesError }}
            </p>
            <p v-else-if="!files.length" class="px-3 py-2 text-xs text-muted-foreground">
              {{ t(isMerge ? "git.graph.detail.mergeCommit" : "git.graph.detail.emptyFiles") }}
            </p>

            <!-- 平铺:只显示文件名,完整路径放 title -->
            <FileTreeList
              v-else-if="!treeMode"
              size="sm"
              flat
              :rows="flatRows"
              :selected="selectedPath"
              @select="(row) => selectFile(row.data!)"
            >
              <template #leading="{ row }">
                <span
                  class="w-3 shrink-0 font-mono font-semibold"
                  :class="statusClass(row.data!.status)"
                >
                  {{ row.data!.status }}
                </span>
              </template>
              <template #trailing="{ row }">
                <span
                  v-if="row.data!.additions"
                  class="shrink-0 text-green-600 dark:text-green-400"
                >
                  +{{ row.data!.additions }}
                </span>
                <span v-if="row.data!.deletions" class="shrink-0 text-red-600 dark:text-red-400">
                  -{{ row.data!.deletions }}
                </span>
              </template>
            </FileTreeList>

            <!-- 树形:按目录层级聚合 -->
            <FileTreeList
              v-else
              size="sm"
              :rows="treeRows"
              :selected="selectedPath"
              @select="(row) => selectFile(row.data!)"
              @toggle="(row) => toggleFolder(row.fullPath)"
            >
              <template #leading="{ row }">
                <span
                  v-if="row.data"
                  class="w-3 shrink-0 font-mono font-semibold"
                  :class="statusClass(row.data.status)"
                >
                  {{ row.data.status }}
                </span>
              </template>
              <template #trailing="{ row }">
                <template v-if="row.data">
                  <span
                    v-if="row.data.additions"
                    class="shrink-0 text-green-600 dark:text-green-400"
                  >
                    +{{ row.data.additions }}
                  </span>
                  <span v-if="row.data.deletions" class="shrink-0 text-red-600 dark:text-red-400">
                    -{{ row.data.deletions }}
                  </span>
                </template>
              </template>
            </FileTreeList>
          </template>
        </div>
      </div>

      <!-- 列表 / diff 分隔拖拽条:上下布局横条调高,左右布局竖条调宽 -->
      <div
        class="shrink-0 transition-colors hover:bg-primary/50"
        :class="layout === 'horizontal' ? 'w-1.5 cursor-col-resize' : 'h-1.5 cursor-row-resize'"
        @pointerdown="startListResize"
      />

      <!-- diff 区:图片文件走 blob 图像预览(新增只看新/删除只看旧/修改左右对照),
           其余与提交对话框变更预览共用 DiffViewer(解析/着色/折叠/并排/导航全在其内) -->
      <ImageDiffPreview
        v-if="selectedIsImage"
        :file-path="selectedPath"
        :loading="imageLoading"
        :error="imageError"
        :panes="imagePanes"
      />
      <DiffViewer
        v-else
        v-model:ignore-ws="ignoreWs"
        :diff="diff"
        :file-path="selectedPath"
        :loading="diffLoading"
        :error="diffError"
        :split-applicable="splitApplicable"
        :can-open-ide="canOpenInIde"
        :reveal="diffReveal"
        @open-ide="selectedFile && openFile(selectedFile)"
      />
    </div>

    <!-- 影响分析:sem 基于当前工作树/HEAD 的实体图,查看历史提交时提示 -->
    <SemanticImpactPanel
      v-model:open="impactOpen"
      :root="projectPath"
      :entity="impactEntity"
      :show-current-code-notice="!commit.is_head"
      @open="openImpactTarget"
    />
  </div>
</template>
