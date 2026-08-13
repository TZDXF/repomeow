<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useLocalStorage } from "@vueuse/core";
import {
  ChevronRight,
  Copy,
  ExternalLink,
  Folder,
  FolderTree,
  GitBranch,
  List,
  Loader2,
  Tag as TagIcon,
  X,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { buildFileTree, type FileTreeNode } from "@/lib/file-tree";
import { cmd } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settings";
import type { GitCommitFile, GitCommitFileDiff, GitGraphCommit } from "@/types";

const props = defineProps<{
  commit: GitGraphCommit;
  /** 项目根目录绝对路径(拼接文件相对路径后用于 IDE 打开) */
  projectPath: string;
}>();

const emit = defineEmits<{
  close: [];
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
  diffLoading.value = true;
  diffError.value = "";
  diff.value = null;
  try {
    diff.value = await cmd<GitCommitFileDiff>("git_commit_file_diff", {
      path: props.projectPath,
      hash: props.commit.hash,
      filePath: file.path,
      oldPath: file.old_path,
    });
  } catch (e) {
    diffError.value = String(e);
  } finally {
    diffLoading.value = false;
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

// --- diff 解析:逐行旧/新行号;文件头(diff --git / index / --- / +++ 等)不展示 ---
interface DiffLine {
  kind: "hunk" | "add" | "del" | "ctx" | "meta";
  text: string;
  oldLine: number | null;
  newLine: number | null;
}

function parseDiff(text: string): DiffLine[] {
  const out: DiffLine[] = [];
  let oldN = 0;
  let newN = 0;
  let seenHunk = false;
  for (const raw of text.split("\n")) {
    if (!raw) {
      continue;
    }
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(raw);
    if (hunk) {
      oldN = Number(hunk[1]);
      newN = Number(hunk[2]);
      seenHunk = true;
      out.push({ kind: "hunk", text: raw, oldLine: null, newLine: null });
      continue;
    }
    // 首个 hunk 之前是文件头(diff --git / index / --- / +++ 等),仅保留二进制提示
    if (!seenHunk) {
      if (raw.startsWith("Binary files")) {
        out.push({ kind: "meta", text: raw, oldLine: null, newLine: null });
      }
      continue;
    }
    if (raw.startsWith("+")) {
      out.push({ kind: "add", text: raw, oldLine: null, newLine: newN });
      newN++;
    } else if (raw.startsWith("-")) {
      out.push({ kind: "del", text: raw, oldLine: oldN, newLine: newN });
      oldN++;
    } else if (raw.startsWith(" ")) {
      out.push({ kind: "ctx", text: raw, oldLine: oldN, newLine: newN });
      oldN++;
      newN++;
    } else {
      // "\ No newline at end of file" 等
      out.push({ kind: "meta", text: raw, oldLine: null, newLine: null });
    }
  }
  return out;
}

const diffLines = computed(() => (diff.value ? parseDiff(diff.value.diff) : []));

/** 当前选中文件(已删除的文件不提供 IDE 打开:工作区已不存在) */
const selectedFile = computed(() => files.value.find((f) => f.path === selectedPath.value) ?? null);
const canOpenInIde = computed(() => selectedFile.value?.status !== "D");

// --- 在 IDE 打开(默认编辑器) ---
async function openFile(file: GitCommitFile) {
  try {
    await cmd("open_in_editor", {
      path: `${props.projectPath}/${file.path}`,
      kind: settings.defaultOpenWith,
      line: null,
    });
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

// --- 文件列表 / diff 区上下分高(拖拽调整) ---
const listHeight = useLocalStorage("repomeow:commit-detail-list-h", 240);
const LIST_MIN_H = 120;
const LIST_MAX_H = 600;

function startListResize(e: PointerEvent) {
  e.preventDefault();
  const startY = e.clientY;
  const startH = listHeight.value;
  const onMove = (ev: PointerEvent) => {
    listHeight.value = Math.min(
      LIST_MAX_H,
      Math.max(LIST_MIN_H, Math.round(startH + ev.clientY - startY)),
    );
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
  <div class="flex h-full flex-col">
    <!-- 提交信息 -->
    <div class="shrink-0 border-b px-3 py-2.5">
      <div class="flex items-start justify-between gap-2">
        <p class="min-w-0 text-sm font-medium break-all">{{ commit.subject }}</p>
        <Button variant="ghost" size="sm" class="h-6 w-6 shrink-0 p-0" @click="emit('close')">
          <X class="h-4 w-4" />
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

    <!-- 文件列表:平铺 / 树形切换 -->
    <div class="flex shrink-0 flex-col" :style="{ height: `${listHeight}px` }">
      <div class="flex shrink-0 items-center justify-between border-b px-3 py-1.5">
        <span class="text-xs font-medium text-muted-foreground">
          {{ t("git.graph.detail.filesCount", { count: files.length }) }}
        </span>
        <button
          class="rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          :title="t(treeMode ? 'git.graph.detail.showFlat' : 'git.graph.detail.showTree')"
          @click="treeMode = !treeMode"
        >
          <List v-if="treeMode" class="h-3.5 w-3.5" />
          <FolderTree v-else class="h-3.5 w-3.5" />
        </button>
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
            class="group flex w-full cursor-pointer items-center gap-1.5 px-3 py-1 text-xs transition-colors hover:bg-accent/60"
            :class="selectedPath === file.path ? 'bg-accent' : ''"
            @click="selectFile(file)"
          >
            <span class="w-3 shrink-0 font-mono font-semibold" :class="statusClass(file.status)">
              {{ file.status }}
            </span>
            <span class="min-w-0 flex-1 truncate font-mono" :title="file.path">
              <template v-if="file.old_path">{{ file.old_path }} → </template>{{ file.path }}
            </span>
            <span v-if="file.additions != null" class="shrink-0 text-green-600 dark:text-green-400">
              +{{ file.additions }}
            </span>
            <span v-if="file.deletions != null" class="shrink-0 text-red-600 dark:text-red-400">
              -{{ file.deletions }}
            </span>
            <button
              v-if="file.status !== 'D'"
              class="shrink-0 rounded-sm p-0.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground"
              :title="t('git.graph.detail.openInIde')"
              @click.stop="openFile(file)"
            >
              <ExternalLink class="h-3 w-3" />
            </button>
          </div>
        </template>

        <!-- 树形 -->
        <template v-else>
          <div
            v-for="row in treeRows"
            :key="row.node.fullPath"
            class="group flex w-full items-center gap-1.5 py-1 pr-3 text-xs transition-colors"
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
            <Folder v-if="!row.node.file" class="h-3 w-3 shrink-0 text-muted-foreground" />
            <span
              v-else
              class="w-3 shrink-0 font-mono font-semibold"
              :class="statusClass(row.node.file.status)"
            >
              {{ row.node.file.status }}
            </span>
            <span class="min-w-0 flex-1 truncate font-mono" :title="row.node.fullPath">
              {{ row.node.name }}
            </span>
            <template v-if="row.node.file">
              <span
                v-if="row.node.file.additions != null"
                class="shrink-0 text-green-600 dark:text-green-400"
              >
                +{{ row.node.file.additions }}
              </span>
              <span
                v-if="row.node.file.deletions != null"
                class="shrink-0 text-red-600 dark:text-red-400"
              >
                -{{ row.node.file.deletions }}
              </span>
              <button
                v-if="row.node.file.status !== 'D'"
                class="shrink-0 rounded-sm p-0.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground"
                :title="t('git.graph.detail.openInIde')"
                @click.stop="openFile(row.node.file!)"
              >
                <ExternalLink class="h-3 w-3" />
              </button>
            </template>
          </div>
        </template>
      </div>
    </div>

    <!-- 列表 / diff 上下分高拖拽条 -->
    <div
      class="h-1.5 shrink-0 cursor-row-resize transition-colors hover:bg-primary/50"
      @pointerdown="startListResize"
    />

    <!-- diff 区:自实现逐行渲染(行号 + 增删底色) -->
    <div class="flex min-h-0 flex-1 flex-col">
      <div class="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
        <span class="min-w-0 flex-1 truncate font-mono text-xs" :title="selectedFile?.path">
          {{ selectedFile?.path ?? "" }}
        </span>
        <Badge v-if="diff?.truncated" variant="outline" class="h-5 shrink-0 px-1.5 text-[10px]">
          {{ t("git.graph.detail.diffTruncated") }}
        </Badge>
        <button
          v-if="selectedFile && canOpenInIde"
          class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          :title="t('git.graph.detail.openInIde')"
          @click="openFile(selectedFile)"
        >
          <ExternalLink class="h-3.5 w-3.5" />
        </button>
      </div>

      <div class="min-h-0 flex-1 overflow-auto">
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

        <div v-else class="min-w-max py-1 font-mono text-xs leading-5">
          <template v-for="(line, i) in diffLines" :key="i">
            <div
              v-if="line.kind === 'hunk'"
              class="bg-muted/60 px-3 text-muted-foreground select-none"
            >
              {{ line.text }}
            </div>
            <div v-else-if="line.kind === 'meta'" class="px-3 text-muted-foreground select-none">
              {{ line.text }}
            </div>
            <div
              v-else
              class="flex w-full"
              :class="
                line.kind === 'add' ? 'bg-green-500/10' : line.kind === 'del' ? 'bg-red-500/10' : ''
              "
            >
              <span class="w-10 shrink-0 pr-2 text-right text-muted-foreground/50 select-none">
                {{ line.oldLine ?? "" }}
              </span>
              <span class="w-10 shrink-0 pr-2 text-right text-muted-foreground/50 select-none">
                {{ line.newLine ?? "" }}
              </span>
              <span class="whitespace-pre">{{ line.text }}</span>
            </div>
          </template>
        </div>
      </div>
    </div>
  </div>
</template>
