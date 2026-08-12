<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  ArrowUpToLine,
  FolderOpen,
  GitBranchPlus,
  GitMerge,
  Loader2,
  Trash2,
  TriangleAlert,
  Undo2,
} from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { GitBranches, GitWorktree, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });
/** worktree 列表发生增删后通知父组件(切换下拉据此刷新并校验当前选中) */
const emit = defineEmits<{ changed: [] }>();

const store = useProjectsStore();
const settings = useSettingsStore();

const worktrees = ref<GitWorktree[]>([]);
const loading = ref(false);
const branches = ref<GitBranches>({ local: [], remote: [] });

// --- 新建 worktree 表单 ---
const createOpen = ref(false);
/** new = 新建分支挂载;existing = 挂载已有分支 */
const branchMode = ref<"new" | "existing">("new");
const newBranch = ref("");
const existingBranch = ref("");
const baseBranch = ref("");
const dirPath = ref("");
/** 用户手动改过目录后不再随分支名联动 */
const dirTouched = ref(false);
const creating = ref(false);

// --- 合回当前分支确认 ---
const mergeTarget = ref<GitWorktree | null>(null);
const mergeSquash = ref(false);
const merging = ref(false);

// --- 删除确认 ---
const removeTarget = ref<GitWorktree | null>(null);
const removeForce = ref(false);
const removeDeleteBranch = ref(false);
const removing = ref(false);

// --- 变基(按 worktree 路径标记进行中/中断) ---
const rebasingPath = ref<string | null>(null);
const rebaseInterruptedPath = ref<string | null>(null);
const aborting = ref(false);

// --- 冲突引导(复用 ConflictDialog;worktree 内冲突传 worktree 路径) ---
const conflictOpen = ref(false);
const conflictFiles = ref<string[]>([]);
const conflictPath = ref<string | undefined>(undefined);

const currentBranch = computed(() => props.project.git?.branch ?? null);

/** 远程分支去掉远端名前缀: "origin/team/x" -> "team/x" */
function remoteShortName(remote: string) {
  return remote.slice(remote.indexOf("/") + 1);
}

// 已被某个 worktree 检出的分支名(挂载已有分支时不可再选)
const checkedOutBranches = computed(() => {
  return new Set(worktrees.value.map((w) => w.branch).filter((b): b is string => !!b));
});

// 可挂载的已有分支:本地/远程均排除已被检出的。远程分支全量列出——本地与远程
// 可能不同步,同名时两者是不同提交,由用户选择挂载哪一侧(后端按远程语义对齐)
const attachableLocal = computed(() =>
  branches.value.local.filter((b) => !checkedOutBranches.value.has(b)),
);
const attachableRemote = computed(() =>
  branches.value.remote.filter((r) => !checkedOutBranches.value.has(remoteShortName(r))),
);

/** 当前选定的挂载分支名(用于目录模板联动与提交校验);
 * 远程引用(origin/x)落地后的本地名是去掉首段前缀的部分 */
const effectiveBranch = computed(() => {
  if (branchMode.value === "new") return newBranch.value.trim();
  const b = existingBranch.value;
  if (!b) return "";
  return branches.value.local.includes(b) ? b : remoteShortName(b);
});

/** 展示用路径:相对主工作区的尽量显示相对路径 */
const mainPath = computed(() => worktrees.value.find((w) => w.is_main)?.path ?? "");
function displayPath(w: GitWorktree) {
  const root = mainPath.value;
  if (root && w.path.startsWith(root)) {
    const rel = w.path.slice(root.length).replace(/^[/\\]/, "");
    if (rel) return rel;
  }
  return w.path;
}

/** 该行是否可合回/变基:游离 HEAD 或与当前分支同名时无意义 */
function canMergeBack(w: GitWorktree) {
  return !!w.branch && w.branch !== currentBranch.value;
}

async function load() {
  loading.value = true;
  try {
    const [wts, brs] = await Promise.all([
      store.listWorktrees(props.project),
      store.listBranches(props.project),
    ]);
    worktrees.value = wts;
    branches.value = brs;
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

watch(open, (v) => {
  if (v) {
    load();
    createOpen.value = false;
    rebaseInterruptedPath.value = null;
  }
});

// 展开新建表单时:基点默认当前分支,目录按设置模板预填
watch(createOpen, (v) => {
  if (!v) return;
  branchMode.value = "new";
  baseBranch.value = currentBranch.value ?? "";
  existingBranch.value = "";
  dirTouched.value = false;
  dirPath.value = applyTemplate(effectiveBranch.value);
});

// 分支名变化且目录未被手动修改时,同步模板中的 {branch}
watch(effectiveBranch, (name) => {
  if (!dirTouched.value) {
    dirPath.value = applyTemplate(name);
  }
});

function applyTemplate(branch: string) {
  return settings.worktreeDirTemplate.replace("{branch}", branch.trim().replace(/\//g, "-"));
}

async function create() {
  const isNew = branchMode.value === "new";
  const name = isNew ? newBranch.value.trim() : existingBranch.value;
  const dir = dirPath.value.trim();
  if (!name || !dir || creating.value) return;
  creating.value = true;
  try {
    const current = currentBranch.value;
    const startPoint =
      isNew && baseBranch.value && baseBranch.value !== current ? baseBranch.value : undefined;
    worktrees.value = await store.addWorktree(props.project, dir, name, {
      createBranch: isNew,
      startPoint,
    });
    toast.success(t("git.worktree.created", { name: effectiveBranch.value || name }));
    emit("changed");
    createOpen.value = false;
    newBranch.value = "";
  } catch (e) {
    toast.error(String(e));
  } finally {
    creating.value = false;
  }
}

async function openDir(w: GitWorktree) {
  try {
    await cmd("open_with", { path: w.path, kind: settings.defaultOpenWith });
  } catch (e) {
    toast.error(String(e));
  }
}

function showConflicts(files: string[], path?: string) {
  conflictFiles.value = files;
  conflictPath.value = path;
  conflictOpen.value = true;
}

async function mergeBack() {
  const w = mergeTarget.value;
  if (!w?.branch || merging.value) return;
  merging.value = true;
  try {
    const conflicts = await store.mergeBranch(props.project, w.branch, {
      squash: mergeSquash.value,
    });
    if (conflicts.length) {
      showConflicts(conflicts);
    } else if (mergeSquash.value) {
      // squash 不自动提交:提示用户确认后走常规提交
      toast.success(t("git.worktree.squashStaged", { name: w.branch }));
    } else {
      toast.success(t("git.worktree.merged", { name: w.branch }));
    }
    mergeTarget.value = null;
  } catch (e) {
    toast.error(String(e));
  } finally {
    merging.value = false;
  }
}

/** 在 worktree 内执行:将该 worktree 的分支变基到主工作区当前分支之上 */
async function rebase(w: GitWorktree) {
  const onto = currentBranch.value;
  if (!onto || rebasingPath.value) return;
  rebasingPath.value = w.path;
  try {
    const result = await store.rebaseBranch(props.project, onto, w.path);
    if (result.inProgress) {
      rebaseInterruptedPath.value = w.path;
      if (result.conflicts.length) {
        showConflicts(result.conflicts, w.path);
      }
      toast.warning(t("git.worktree.rebaseInterrupted"));
    } else {
      rebaseInterruptedPath.value = null;
      toast.success(t("git.worktree.rebased", { name: w.branch ?? w.head.slice(0, 7) }));
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    rebasingPath.value = null;
  }
}

async function abortRebase(w: GitWorktree) {
  if (aborting.value) return;
  aborting.value = true;
  try {
    await store.abortRebase(props.project, w.path);
    rebaseInterruptedPath.value = null;
    toast.success(t("git.worktree.aborted"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    aborting.value = false;
  }
}

/** 主工作区合并冲突进行中时提供兜底中止入口 */
async function abortMerge() {
  if (aborting.value) return;
  aborting.value = true;
  try {
    await store.abortMerge(props.project);
    toast.success(t("git.worktree.aborted"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    aborting.value = false;
  }
}

async function remove() {
  const w = removeTarget.value;
  if (!w || removing.value) return;
  removing.value = true;
  try {
    worktrees.value = await store.removeWorktree(props.project, w.path, {
      force: removeForce.value,
      deleteBranch: removeDeleteBranch.value,
    });
    toast.success(t("git.worktree.removed"));
    emit("changed");
    removeTarget.value = null;
  } catch (e) {
    toast.error(String(e));
  } finally {
    removing.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2">
          {{ t("git.worktree.title") }}
          <Badge v-if="worktrees.length" variant="secondary" class="font-normal">
            {{ t("git.worktree.count", { count: worktrees.length }) }}
          </Badge>
        </DialogTitle>
      </DialogHeader>

      <!-- 主工作区合并冲突进行中:提供中止入口 -->
      <div
        v-if="(project.git?.conflicted ?? 0) > 0"
        class="flex items-center gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs"
      >
        <TriangleAlert class="h-3.5 w-3.5 shrink-0 text-amber-500" />
        <span class="flex-1">{{ t("git.worktree.mergeConflictPending") }}</span>
        <Button variant="outline" size="xs" :disabled="aborting" @click="abortMerge">
          <Undo2 class="h-3 w-3" />
          {{ t("git.worktree.abortMerge") }}
        </Button>
      </div>

      <div v-if="loading" class="flex items-center gap-2 py-6 text-xs text-muted-foreground">
        <Loader2 class="h-3.5 w-3.5 animate-spin" />
        {{ t("common.loading") }}
      </div>

      <ScrollArea v-else class="max-h-72">
        <ul class="flex flex-col gap-1 pr-3">
          <li
            v-for="w in worktrees"
            :key="w.path"
            class="flex items-center gap-2 rounded-md border px-3 py-2"
          >
            <div class="min-w-0 flex-1">
              <div class="flex items-center gap-1.5">
                <span class="truncate text-sm font-medium">
                  {{ w.branch ?? w.head.slice(0, 7) }}
                </span>
                <Badge v-if="w.is_main" variant="secondary" class="text-[10px]">
                  {{ t("git.worktree.main") }}
                </Badge>
                <Badge v-else-if="w.detached" variant="outline" class="text-[10px]">
                  {{ t("git.worktree.detached") }}
                </Badge>
              </div>
              <p class="mt-0.5 truncate font-mono text-xs text-muted-foreground" :title="w.path">
                {{ displayPath(w) }}
              </p>
            </div>
            <div class="flex shrink-0 items-center gap-1">
              <Button variant="ghost" size="xs" :title="t('git.worktree.open')" @click="openDir(w)">
                <FolderOpen class="h-3.5 w-3.5" />
              </Button>
              <template v-if="!w.is_main">
                <Button
                  variant="ghost"
                  size="xs"
                  :disabled="!canMergeBack(w)"
                  :title="t('git.worktree.mergeBack')"
                  @click="((mergeTarget = w), (mergeSquash = false))"
                >
                  <GitMerge class="h-3.5 w-3.5" />
                </Button>
                <Button
                  v-if="rebaseInterruptedPath === w.path"
                  variant="ghost"
                  size="xs"
                  class="text-amber-600"
                  :disabled="aborting"
                  :title="t('git.worktree.abortRebase')"
                  @click="abortRebase(w)"
                >
                  <Undo2 class="h-3.5 w-3.5" />
                </Button>
                <Button
                  v-else
                  variant="ghost"
                  size="xs"
                  :disabled="!canMergeBack(w) || rebasingPath === w.path"
                  :title="t('git.worktree.rebase')"
                  @click="rebase(w)"
                >
                  <Loader2 v-if="rebasingPath === w.path" class="h-3.5 w-3.5 animate-spin" />
                  <ArrowUpToLine v-else class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="xs"
                  class="text-destructive"
                  :title="t('git.worktree.remove')"
                  @click="((removeTarget = w), (removeForce = false), (removeDeleteBranch = false))"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </template>
            </div>
          </li>
        </ul>
      </ScrollArea>

      <Separator />

      <!-- 新建 worktree -->
      <div v-if="!createOpen" class="flex justify-end">
        <Button variant="outline" size="sm" @click="createOpen = true">
          <GitBranchPlus class="h-3.5 w-3.5" />
          {{ t("git.worktree.create") }}
        </Button>
      </div>
      <form v-else class="flex flex-col gap-3" @submit.prevent="create">
        <div class="grid grid-cols-2 gap-3">
          <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
            {{ t("git.worktree.sourceLabel") }}
            <Select v-model="branchMode">
              <SelectTrigger class="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="new">{{ t("git.worktree.sourceNew") }}</SelectItem>
                <SelectItem value="existing">{{ t("git.worktree.sourceExisting") }}</SelectItem>
              </SelectContent>
            </Select>
          </label>
          <label
            v-if="branchMode === 'new'"
            class="flex flex-col gap-1.5 text-xs text-muted-foreground"
          >
            {{ t("git.worktree.branchLabel") }}
            <Input
              v-model="newBranch"
              :placeholder="t('git.worktree.branchPlaceholder')"
              autofocus
            />
          </label>
          <label v-else class="flex flex-col gap-1.5 text-xs text-muted-foreground">
            {{ t("git.worktree.existingBranchLabel") }}
            <Select v-model="existingBranch">
              <SelectTrigger class="w-full">
                <SelectValue :placeholder="t('git.worktree.existingBranchLabel')" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup v-if="attachableLocal.length">
                  <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
                  <SelectItem v-for="b in attachableLocal" :key="b" :value="b">
                    {{ b }}
                  </SelectItem>
                </SelectGroup>
                <SelectGroup v-if="attachableRemote.length">
                  <SelectLabel>{{ t("git.branch.remote") }}</SelectLabel>
                  <SelectItem v-for="r in attachableRemote" :key="r" :value="r">
                    {{ r }}
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </label>
          <label
            v-if="branchMode === 'new'"
            class="col-span-2 flex flex-col gap-1.5 text-xs text-muted-foreground"
          >
            {{ t("git.worktree.baseLabel") }}
            <Select v-model="baseBranch">
              <SelectTrigger class="w-full">
                <SelectValue :placeholder="t('git.worktree.baseLabel')" />
              </SelectTrigger>
              <SelectContent>
                <SelectGroup>
                  <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
                  <SelectItem v-for="b in branches.local" :key="b" :value="b">
                    {{ b }}
                  </SelectItem>
                </SelectGroup>
                <SelectGroup v-if="branches.remote.length">
                  <SelectLabel>{{ t("git.branch.remote") }}</SelectLabel>
                  <SelectItem v-for="r in branches.remote" :key="r" :value="r">
                    {{ r }}
                  </SelectItem>
                </SelectGroup>
              </SelectContent>
            </Select>
          </label>
        </div>
        <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
          {{ t("git.worktree.pathLabel") }}
          <Input v-model="dirPath" class="font-mono" @input="dirTouched = true" />
          <span class="text-[11px]">{{ t("git.worktree.pathHint") }}</span>
        </label>
        <div class="flex justify-end gap-2">
          <Button type="button" variant="ghost" size="sm" @click="createOpen = false">
            {{ t("common.cancel") }}
          </Button>
          <Button
            type="submit"
            size="sm"
            :disabled="!effectiveBranch || !dirPath.trim() || creating"
          >
            {{ creating ? t("git.worktree.creating") : t("common.create") }}
          </Button>
        </div>
      </form>
    </DialogContent>
  </Dialog>

  <!-- 合回当前分支确认(可选 squash) -->
  <Dialog :open="!!mergeTarget" @update:open="mergeTarget = null">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>
          {{ t("git.worktree.mergeTitle", { name: mergeTarget?.branch ?? "" }) }}
        </DialogTitle>
      </DialogHeader>
      <label class="flex items-center gap-2 text-sm">
        <Switch v-model="mergeSquash" />
        {{ t("git.worktree.squash") }}
      </label>
      <DialogFooter class="gap-2">
        <Button variant="ghost" @click="mergeTarget = null">{{ t("common.cancel") }}</Button>
        <Button :disabled="merging" @click="mergeBack">
          {{ merging ? t("git.worktree.merging") : t("git.worktree.mergeBack") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <!-- 删除确认(可选强制/删分支) -->
  <Dialog :open="!!removeTarget" @update:open="removeTarget = null">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.worktree.removeTitle") }}</DialogTitle>
      </DialogHeader>
      <p class="text-sm text-muted-foreground">
        {{ t("git.worktree.removeConfirm", { path: removeTarget?.path ?? "" }) }}
      </p>
      <div class="flex flex-col gap-2">
        <label class="flex items-center gap-2 text-sm">
          <Switch v-model="removeForce" />
          {{ t("git.worktree.removeForce") }}
        </label>
        <label v-if="removeTarget?.branch" class="flex items-center gap-2 text-sm">
          <Switch v-model="removeDeleteBranch" />
          {{ t("git.worktree.removeDeleteBranch", { name: removeTarget.branch }) }}
        </label>
      </div>
      <DialogFooter class="gap-2">
        <Button variant="ghost" @click="removeTarget = null">{{ t("common.cancel") }}</Button>
        <Button variant="destructive" :disabled="removing" @click="remove">
          {{ removing ? t("git.worktree.removing") : t("git.worktree.remove") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <ConflictDialog
    v-model:open="conflictOpen"
    :project="project"
    :conflicts="conflictFiles"
    :path="conflictPath"
  />
</template>
