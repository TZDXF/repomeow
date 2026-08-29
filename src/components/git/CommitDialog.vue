<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useLocalStorage } from "@vueuse/core";
import { ChevronDown, FileDiff, FolderTree, List, Loader2, Sparkles } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import FileTreeList from "@/components/common/FileTreeList.vue";
import DiffViewer from "@/components/git/DiffViewer.vue";
import { generateCommitMessage } from "@/lib/ai";
import { statusClass } from "@/lib/git-status";
import {
  buildFileTree,
  flatFileRows,
  flattenVisibleTree,
  type FileTreeNode,
} from "@/lib/file-tree";
import { openPathWith, sortOpenWithOptions } from "@/lib/open-with";
import { baseName } from "@/lib/path";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { GitCommitFileDiff, GitWorktreeFile, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });
const store = useProjectsStore();
const settings = useSettingsStore();

const message = ref("");
const submitting = ref(false);
const submittingAndPushing = ref(false);
const generating = ref(false);
// 未跟踪文件默认勾选纳入本次提交
const includeUntracked = ref(true);

const git = computed(() => props.project.git);
const untrackedCount = computed(() => git.value?.untracked ?? 0);

// --- 变更预览:默认折叠,点击展开后加载文件清单与单文件 diff(相对 HEAD,含未跟踪文件) ---
const showChanges = ref(false);
const files = ref<GitWorktreeFile[]>([]);
const filesLoading = ref(false);
const filesError = ref("");
const selectedPath = ref<string | null>(null);
const diff = ref<GitCommitFileDiff | null>(null);
const diffLoading = ref(false);
const diffError = ref("");

/** 忽略空白差异模式(与提交详情面板同键同源):none 不忽略 / eol 行尾 / change 空白数量变化 / all 全部空白 */
const ignoreWs = useLocalStorage<"none" | "eol" | "change" | "all">(
  "repomeow:commit-diff-ignore-ws",
  "none",
);

// --- 文件勾选:可提交子集;未跟踪文件仅在勾选"包含未跟踪文件"时可提交 ---
const checkedPaths = ref(new Set<string>());
/** 用户主动取消勾选的路径:收起再展开重新拉取清单时保留取消意图(新出现的文件默认勾选) */
const deselectedPaths = ref(new Set<string>());

/** 未勾选"包含未跟踪文件"时未跟踪文件不可提交(勾选框禁用) */
function fileCommittable(file: GitWorktreeFile) {
  return !file.untracked || includeUntracked.value;
}

const committableFiles = computed(() => files.value.filter(fileCommittable));
const checkedFiles = computed(() =>
  committableFiles.value.filter((f) => checkedPaths.value.has(f.path)),
);
const allChecked = computed(
  () =>
    committableFiles.value.length > 0 &&
    checkedFiles.value.length === committableFiles.value.length,
);
const someChecked = computed(() => checkedFiles.value.length > 0);

/** 本次实际会提交的变更数:展开过预览(清单已加载)时按勾选数计,否则按状态计数 */
const committable = computed(() => {
  if (files.value.length) {
    return checkedFiles.value.length;
  }
  if (!git.value) {
    return 0;
  }
  return git.value.staged + git.value.modified + (includeUntracked.value ? git.value.untracked : 0);
});

/** 勾选子集时提交的路径清单(重命名新旧路径一并传入);全选 / 未展开为 null,走后端全量语义 */
const checkedPayload = computed<string[] | null>(() => {
  if (!files.value.length || allChecked.value) {
    return null;
  }
  const out: string[] = [];
  for (const f of checkedFiles.value) {
    if (f.old_path) {
      out.push(f.old_path);
    }
    out.push(f.path);
  }
  return out;
});

/** 勾选 / 取消一组路径,并同步记录取消意图(供重新加载清单时恢复) */
function setChecked(paths: string[], checked: boolean) {
  const nextChecked = new Set(checkedPaths.value);
  const nextDeselected = new Set(deselectedPaths.value);
  for (const p of paths) {
    if (checked) {
      nextChecked.add(p);
      nextDeselected.delete(p);
    } else {
      nextChecked.delete(p);
      nextDeselected.add(p);
    }
  }
  checkedPaths.value = nextChecked;
  deselectedPaths.value = nextDeselected;
}

function toggleFileChecked(file: GitWorktreeFile) {
  setChecked([file.path], !checkedPaths.value.has(file.path));
}

function toggleAllChecked() {
  setChecked(
    committableFiles.value.map((f) => f.path),
    !allChecked.value,
  );
}

// "包含未跟踪文件"勾选联动:未跟踪文件随之取消 / 恢复;
// 取消不算用户主动反选(不记 deselected),恢复时仅补回未被主动取消的
watch(includeUntracked, (v) => {
  if (!files.value.length) {
    return;
  }
  const next = new Set(checkedPaths.value);
  for (const f of files.value) {
    if (!f.untracked) {
      continue;
    }
    if (v) {
      if (!deselectedPaths.value.has(f.path)) {
        next.add(f.path);
      }
    } else {
      next.delete(f.path);
    }
  }
  checkedPaths.value = next;
});

// --- 树形展示:按目录层级聚合,折叠状态记忆;行化(树形/平铺)走 file-tree 统一辅助 ---
// 平铺 / 树形切换与提交详情面板共用持久化键
const treeMode = useLocalStorage("repomeow:commit-files-tree", false);
const collapsedFolders = ref(new Set<string>());
const fileTree = computed(() => buildFileTree(files.value));

const treeRows = computed(() =>
  flattenVisibleTree(fileTree.value, collapsedFolders.value, {
    dim: fileDimmed,
    title: fileTitle,
  }),
);

const flatRows = computed(() =>
  flatFileRows(files.value, {
    name: flatName,
    dim: fileDimmed,
    title: fileTitle,
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

/** 目录节点勾选信息(仅统计可提交文件;total = 0 时勾选框禁用),paths 供整目录勾选 */
const folderInfo = computed(() => {
  const map = new Map<string, { all: boolean; some: boolean; total: number; paths: string[] }>();
  const walk = (
    node: FileTreeNode<GitWorktreeFile>,
  ): { checked: number; total: number; paths: string[] } => {
    let total = 0;
    let checked = 0;
    const paths: string[] = [];
    if (node.file && fileCommittable(node.file)) {
      total = 1;
      checked = checkedPaths.value.has(node.file.path) ? 1 : 0;
      paths.push(node.file.path);
    }
    for (const c of node.children) {
      const r = walk(c);
      total += r.total;
      checked += r.checked;
      paths.push(...r.paths);
    }
    if (!node.file && node.children.length) {
      map.set(node.fullPath, {
        all: total > 0 && checked === total,
        some: checked > 0,
        total,
        paths,
      });
    }
    return { checked, total, paths };
  };
  for (const n of fileTree.value) {
    walk(n);
  }
  return map;
});

/** 目录勾选:全选则整目录取消,否则整目录选中(只影响可提交文件) */
function toggleFolderChecked(fullPath: string) {
  const info = folderInfo.value.get(fullPath);
  if (!info || info.total === 0) {
    return;
  }
  setChecked(info.paths, !info.all);
}

/** 未勾选"包含未跟踪文件"时未跟踪文件不纳入本次提交,列表中降透明度提示 */
function fileDimmed(file: GitWorktreeFile) {
  return file.untracked && !includeUntracked.value;
}

/** 行 title:重命名文件展示「旧路径 → 新路径」 */
function fileTitle(file: GitWorktreeFile) {
  return file.old_path ? `${file.old_path} → ${file.path}` : file.path;
}

/** 平铺模式只显示文件名(重命名带箭头);baseName 走 @/lib/path */
function flatName(file: GitWorktreeFile) {
  return file.old_path
    ? `${baseName(file.old_path)} → ${baseName(file.path)}`
    : baseName(file.path);
}

const selectedFile = computed(() => files.value.find((f) => f.path === selectedPath.value) ?? null);
const canOpenInIde = computed(() => !!selectedFile.value && selectedFile.value.status !== "D");
/** 并排是否适用:新增/删除文件一侧必然全空,强制逐行视图(与提交详情面板一致) */
const splitApplicable = computed(
  () => selectedFile.value?.status !== "A" && selectedFile.value?.status !== "D",
);

async function loadChanges() {
  filesLoading.value = true;
  filesError.value = "";
  files.value = [];
  selectedPath.value = null;
  diff.value = null;
  diffError.value = "";
  try {
    files.value = await cmd<GitWorktreeFile[]>("git_worktree_files", {
      path: props.project.path,
    });
    // 可提交文件默认勾选;保留用户之前主动取消的选择(收起再展开不丢),新出现的文件默认勾选
    checkedPaths.value = new Set(
      committableFiles.value.filter((f) => !deselectedPaths.value.has(f.path)).map((f) => f.path),
    );
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

async function selectFile(file: GitWorktreeFile, force = false) {
  if (!force && selectedPath.value === file.path && diff.value) {
    return;
  }
  selectedPath.value = file.path;
  // 切文件不清空旧 diff、不转圈:旧内容保留到新结果落地后由 DiffViewer 同帧替换(与提交详情面板一致);
  // 仅首次加载(没有任何旧内容)时 diffLoading 才为 true,给空区域一个转圈
  diffLoading.value = !diff.value;
  diffError.value = "";
  try {
    const result = await cmd<GitCommitFileDiff>("git_worktree_file_diff", {
      path: props.project.path,
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

// 忽略空白模式变化:按新模式重取当前文件 diff(行集会变)
watch(ignoreWs, () => {
  if (selectedFile.value) void selectFile(selectedFile.value, true);
});

/** 在 IDE 打开(默认编辑器;未跟踪文件工作区已存在,同样可打开) */
async function openFile(file: GitWorktreeFile) {
  const option = sortOpenWithOptions(settings.openWithOrder, settings.customOpenWith).find(
    (candidate) => candidate.id === settings.defaultOpenWith,
  );
  if (!option) return;
  try {
    await openPathWith(option, `${props.project.path}/${file.path}`);
  } catch (e) {
    toast.error(String(e));
  }
}

/** 展开 / 收起变更预览;每次展开重新拉取(工作区可能已变化) */
function toggleChanges() {
  showChanges.value = !showChanges.value;
  if (showChanges.value) {
    void loadChanges();
  }
}

// 每次打开时重置为初始状态(清空预览清单与取消记录,避免上次的勾选状态影响提交计数);
// message 不在此清空:AI 生成或手动输入的提交信息在关闭再打开后保留,仅提交成功后清空
watch(open, (v) => {
  if (v) {
    includeUntracked.value = true;
    showChanges.value = false;
    files.value = [];
    checkedPaths.value = new Set();
    deselectedPaths.value = new Set();
  }
});

async function submit() {
  if (!message.value.trim() || committable.value === 0 || submitting.value) return;
  submitting.value = true;
  try {
    await store.commitChanges(
      props.project,
      message.value.trim(),
      includeUntracked.value,
      checkedPayload.value,
    );
    toast.success(t("git.commit.success"));
    message.value = "";
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
  }
}

async function submitAndPush() {
  if (!message.value.trim() || committable.value === 0 || submitting.value) return;
  submitting.value = true;
  submittingAndPushing.value = true;
  try {
    await store.commitChanges(
      props.project,
      message.value.trim(),
      includeUntracked.value,
      checkedPayload.value,
    );
    await store.pushRepository(props.project);
    toast.success(t("git.commit.submitAndPushSuccess"));
    message.value = "";
    open.value = false;
  } catch (e) {
    toast.error(String(e));
  } finally {
    submitting.value = false;
    submittingAndPushing.value = false;
  }
}

/** AI 生成提交信息:严格跟随本次提交的未跟踪开关与文件勾选范围 */
async function generate() {
  if (generating.value || committable.value === 0) return;
  generating.value = true;
  try {
    message.value = await generateCommitMessage(
      props.project,
      settings.language,
      includeUntracked.value,
      checkedPayload.value,
    );
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e));
  } finally {
    generating.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent :class="showChanges ? 'sm:max-w-4xl' : ''">
      <DialogHeader>
        <DialogTitle>{{ t("git.commit.title") }}</DialogTitle>
        <DialogDescription>{{ t("git.commit.description") }}</DialogDescription>
      </DialogHeader>
      <!-- min-w-0:DialogContent 是 grid,表单子项默认按 min-content 撑宽,
           diff 长行会把内容顶出对话框;置 0 后收缩链路生效(预览区 overflow-hidden,diff 自滚动) -->
      <form class="flex min-w-0 flex-col gap-4" @submit.prevent="submit">
        <div v-if="git" class="flex items-center gap-3 text-xs text-muted-foreground">
          <span>
            {{ t("git.staged") }}
            <span class="font-medium text-emerald-600">{{ git.staged }}</span>
          </span>
          <span>
            {{ t("git.modified") }}
            <span class="font-medium text-amber-600">{{ git.modified }}</span>
          </span>
          <span>
            {{ t("git.untracked") }}
            <span class="font-medium text-sky-600">{{ git.untracked }}</span>
          </span>
          <button
            type="button"
            class="ml-auto flex items-center gap-1 rounded-sm px-1.5 py-0.5 transition-colors hover:bg-accent hover:text-foreground"
            :title="t(showChanges ? 'git.commit.hideChanges' : 'git.commit.showChanges')"
            @click="toggleChanges"
          >
            <FileDiff class="h-3.5 w-3.5" />
            {{ t(showChanges ? "git.commit.hideChanges" : "git.commit.showChanges") }}
            <span v-if="files.length && !allChecked">
              ({{ checkedFiles.length }}/{{ committableFiles.length }})
            </span>
            <ChevronDown
              class="h-3 w-3 transition-transform"
              :class="showChanges ? 'rotate-180' : ''"
            />
          </button>
        </div>

        <!-- 变更预览:文件列表 + 单文件 diff(默认折叠) -->
        <div v-if="showChanges" class="flex h-80 overflow-hidden rounded-md border">
          <!-- 文件列表 -->
          <div class="flex w-60 shrink-0 flex-col border-r">
            <div class="flex shrink-0 items-center gap-2 border-b px-3 py-1.5">
              <input
                type="checkbox"
                class="h-3 w-3 shrink-0 accent-primary"
                :checked="allChecked"
                :indeterminate.prop="someChecked && !allChecked"
                :disabled="!committableFiles.length"
                :title="t('git.commit.selectAll')"
                @change="toggleAllChecked"
              />
              <span class="min-w-0 flex-1 text-xs font-medium text-muted-foreground">
                {{ t("git.graph.detail.filesCount", { count: files.length }) }}
              </span>
              <button
                type="button"
                class="shrink-0 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
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
                {{ t("git.graph.detail.emptyFiles") }}
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
                  <input
                    type="checkbox"
                    class="h-3 w-3 shrink-0 accent-primary"
                    :checked="checkedPaths.has(row.data!.path)"
                    :disabled="!fileCommittable(row.data!)"
                    :title="t('git.commit.includeFile')"
                    @click.stop
                    @change="toggleFileChecked(row.data!)"
                  />
                  <span
                    class="w-3 shrink-0 font-mono font-semibold"
                    :class="statusClass(row.data!.untracked ? 'U' : row.data!.status)"
                  >
                    {{ row.data!.untracked ? "U" : row.data!.status }}
                  </span>
                </template>
                <template #trailing="{ row }">
                  <span
                    v-if="row.data!.additions != null"
                    class="shrink-0 text-green-600 dark:text-green-400"
                  >
                    +{{ row.data!.additions }}
                  </span>
                  <span
                    v-if="row.data!.deletions != null"
                    class="shrink-0 text-red-600 dark:text-red-400"
                  >
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
                  <input
                    v-if="row.isDir"
                    type="checkbox"
                    class="h-3 w-3 shrink-0 accent-primary"
                    :checked="folderInfo.get(row.fullPath)?.all ?? false"
                    :indeterminate.prop="
                      (folderInfo.get(row.fullPath)?.some ?? false) &&
                      !(folderInfo.get(row.fullPath)?.all ?? false)
                    "
                    :disabled="!folderInfo.get(row.fullPath)?.total"
                    :title="t('git.commit.selectAll')"
                    @click.stop
                    @change="toggleFolderChecked(row.fullPath)"
                  />
                  <template v-else>
                    <input
                      type="checkbox"
                      class="h-3 w-3 shrink-0 accent-primary"
                      :checked="checkedPaths.has(row.data!.path)"
                      :disabled="!fileCommittable(row.data!)"
                      :title="t('git.commit.includeFile')"
                      @click.stop
                      @change="toggleFileChecked(row.data!)"
                    />
                    <span
                      class="w-3 shrink-0 font-mono font-semibold"
                      :class="statusClass(row.data!.untracked ? 'U' : row.data!.status)"
                    >
                      {{ row.data!.untracked ? "U" : row.data!.status }}
                    </span>
                  </template>
                </template>
                <template #trailing="{ row }">
                  <template v-if="row.data">
                    <span
                      v-if="row.data.additions != null"
                      class="shrink-0 text-green-600 dark:text-green-400"
                    >
                      +{{ row.data.additions }}
                    </span>
                    <span
                      v-if="row.data.deletions != null"
                      class="shrink-0 text-red-600 dark:text-red-400"
                    >
                      -{{ row.data.deletions }}
                    </span>
                  </template>
                </template>
              </FileTreeList>
            </div>
          </div>

          <!-- diff 区:与提交详情面板共用的 DiffViewer(解析/着色/折叠/并排/导航全在其内) -->
          <DiffViewer
            v-model:ignore-ws="ignoreWs"
            :diff="diff"
            :file-path="selectedPath"
            :loading="diffLoading"
            :error="diffError"
            :split-applicable="splitApplicable"
            :can-open-ide="canOpenInIde"
            @open-ide="selectedFile && openFile(selectedFile)"
          />
        </div>

        <div class="flex flex-col gap-1.5">
          <div class="flex items-center justify-between">
            <label class="text-sm font-medium">{{ t("git.commit.messageLabel") }}</label>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              class="h-7 w-7 text-muted-foreground hover:text-foreground"
              :title="generating ? t('git.commit.generating') : t('git.commit.generate')"
              :disabled="generating || committable === 0"
              @click="generate"
            >
              <Loader2 v-if="generating" class="h-3.5 w-3.5 animate-spin" />
              <Sparkles v-else class="h-3.5 w-3.5" />
            </Button>
          </div>
          <textarea
            v-model="message"
            rows="3"
            :placeholder="t('git.commit.messagePlaceholder')"
            class="w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-1 focus-visible:ring-ring"
            autofocus
            @keydown.enter.ctrl.prevent="submit"
          />
        </div>
        <label
          class="flex w-fit items-center gap-2 text-sm"
          :class="untrackedCount === 0 ? 'cursor-not-allowed opacity-50' : 'cursor-pointer'"
        >
          <input
            v-model="includeUntracked"
            type="checkbox"
            :disabled="untrackedCount === 0"
            class="h-3.5 w-3.5 accent-primary"
          />
          {{ t("git.commit.includeUntracked") }}
          <span class="text-xs text-muted-foreground">({{ untrackedCount }})</span>
        </label>
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            :disabled="!message.trim() || committable === 0 || submitting"
            @click="submitAndPush"
          >
            {{
              submittingAndPushing
                ? t("git.commit.submittingAndPushing")
                : t("git.commit.submitAndPush")
            }}
          </Button>
          <Button type="submit" :disabled="!message.trim() || committable === 0 || submitting">
            {{
              submitting && !submittingAndPushing
                ? t("git.commit.submitting")
                : t("git.actions.commit")
            }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
