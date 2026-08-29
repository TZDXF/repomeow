<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  ArchiveRestore,
  ArrowDownToLine,
  ArrowUpToLine,
  Check,
  GitBranchPlus,
  GitCommitHorizontal,
  GitMerge,
  Loader2,
  Radar,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { buildBranchTree, type BranchTreeNode } from "@/lib/branch-tree";
import { toForwardSlash } from "@/lib/path";
import { useProjectsStore } from "@/stores/projects";
import CommitDialog from "@/components/git/CommitDialog.vue";
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import GitBranchDeleteDialog from "@/components/git/GitBranchDeleteDialog.vue";
import GitBranchTreeItems from "@/components/git/GitBranchTreeItems.vue";
import WorktreeOpsDialogs from "@/components/git/WorktreeOpsDialogs.vue";
import type { GitBranches, GitWorktree, Project } from "@/types";

type Op = "pull" | "push" | "";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const emit = defineEmits<{ openStash: [] }>();
const store = useProjectsStore();

const git = computed(() => props.project.git);

const open = ref(false);
const branches = ref<GitBranches>({ local: [], remote: [], tracking: [] });
const worktrees = ref<GitWorktree[]>([]);
const loading = ref(false);
const switching = ref(false);

/** 当前目录若是仓库的 linked worktree 则进入 worktree 模式:切换分支会与其他
 * 工作区冲突,菜单改为提供合回/变基,不展示新建分支与本地/远程分支列表 */
const currentWorktree = computed(() => {
  const p = toForwardSlash(props.project.path);
  return worktrees.value.find((w) => !w.is_main && toForwardSlash(w.path) === p) ?? null;
});
/** 来源分支领先当前 worktree 的提交数(>0 时变基项带计数) */
const baseBehind = computed(() => currentWorktree.value?.base_behind ?? 0);

// --- 当前分支操作(提交/拉取/推送,从原 GitActions 并入) ---
const busy = ref<Op>("");
const commitOpen = ref(false);
const conflictOpen = ref(false);
const conflicts = ref<string[]>([]);

const staged = computed(() => git.value?.staged ?? 0);
const modified = computed(() => git.value?.modified ?? 0);
const untracked = computed(() => git.value?.untracked ?? 0);
const ahead = computed(() => git.value?.ahead ?? 0);
const behind = computed(() => git.value?.behind ?? 0);
const hasChanges = computed(
  () => !!git.value && staged.value + modified.value + untracked.value > 0,
);

/** 提交项悬浮提示:三类变更明细 */
const commitTitle = computed(
  () =>
    `${t("git.staged")} ${staged.value} · ${t("git.modified")} ${modified.value} · ${t("git.untracked")} ${untracked.value}`,
);

/** 任一 git 操作进行中时锁定菜单内全部操作项 */
const opsLocked = computed(() => busy.value !== "" || switching.value || !!branchOp.value);

/** 暴露给触发按钮的同步状态:点击菜单项后菜单即关闭,loading 需展示在外部按钮上 */
const triggerOp = computed<Op>(() => busy.value || branchOp.value?.op || "");

// --- 新建分支对话框 ---
const createOpen = ref(false);
const newBranch = ref("");
const baseBranch = ref("");
const creating = ref(false);

/** 分支名 → upstream 跟踪差值 */
const trackByName = computed(() => {
  const m = new Map<string, { ahead: number; behind: number }>();
  for (const tr of branches.value.tracking) {
    m.set(tr.name, { ahead: tr.ahead, behind: tr.behind });
  }
  return m;
});

/** 本地/远程分支按 "/" 聚合为树:目录内联展开/收起,分支点击展开操作子菜单 */
const localTree = computed(() => buildBranchTree(branches.value.local));
const remoteTree = computed(() => buildBranchTree(branches.value.remote));

/** 树内已折叠的目录键("local:feature" / "remote:origin/feature") */
const collapsedFolders = ref<Set<string>>(new Set());

/** 默认折叠全部目录;本地树展开当前分支的祖先目录,保证当前分支节点可见 */
function defaultCollapsed(): Set<string> {
  const keys = new Set<string>();
  const collect = (nodes: BranchTreeNode[], group: string) => {
    for (const n of nodes) {
      if (!n.children.length) continue;
      keys.add(`${group}:${n.fullPath}`);
      collect(n.children, group);
    }
  };
  collect(localTree.value, "local");
  collect(remoteTree.value, "remote");
  const current = git.value?.branch;
  if (current) {
    const segs = current.split("/");
    let prefix = "";
    for (let i = 0; i < segs.length - 1; i++) {
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      keys.delete(`local:${prefix}`);
    }
  }
  return keys;
}

function toggleFolder(key: string) {
  const next = new Set(collapsedFolders.value);
  if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  collapsedFolders.value = next;
}

/** 远程分支去掉远端名前缀: "origin/team/x" -> "team/x" */
function remoteShortName(remote: string) {
  return remote.slice(remote.indexOf("/") + 1);
}

// 新建分支基点的远程候选:已有本地同名分支的不重复展示(本地分支优先)
const remoteOnly = computed(() => {
  const local = new Set(branches.value.local);
  return branches.value.remote.filter((r) => !local.has(remoteShortName(r)));
});

async function loadBranches() {
  loading.value = true;
  try {
    branches.value = await store.listBranches(props.project);
  } catch (e) {
    toast.error(String(e));
  } finally {
    loading.value = false;
  }
}

/** worktree 列表(判定 worktree 模式与来源分支领先数);失败保持空列表即普通模式 */
async function loadWorktrees() {
  try {
    worktrees.value = await store.listWorktrees(props.project);
  } catch {
    worktrees.value = [];
  }
}

// 挂载即预取 worktree 列表,保证首次展开菜单时 worktree 模式已知,不会先闪出分支列表
watch(() => props.project.path, loadWorktrees, { immediate: true });

// 每次展开菜单时拉取最新分支与 worktree 列表,并重置树折叠状态(默认折叠,仅展开当前分支路径)
watch(open, async (v) => {
  if (!v) {
    return;
  }
  await Promise.all([loadBranches(), loadWorktrees()]);
  collapsedFolders.value = defaultCollapsed();
});

// 打开新建对话框时确保分支列表可用,基点默认当前分支
watch(createOpen, (v) => {
  if (!v) {
    return;
  }
  baseBranch.value = props.project.git?.branch ?? "";
  if (!branches.value.local.length && !branches.value.remote.length && !loading.value) {
    loadBranches();
  }
});

async function switchTo(branch: string, remote = false): Promise<boolean> {
  if (switching.value || (!remote && branch === props.project.git?.branch)) return false;
  switching.value = true;
  try {
    await store.checkoutBranch(props.project, branch, { remote });
    const shown = remote ? remoteShortName(branch) : branch;
    toast.success(t("git.branch.switched", { name: shown }));
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  } finally {
    switching.value = false;
  }
}

/** 树内签出;远程签出可能新建本地跟踪分支,成功后刷新分支列表 */
async function checkoutBranch(branch: string, remote: boolean) {
  if (await switchTo(branch, remote)) {
    loadBranches();
  }
}

async function createBranch() {
  const name = newBranch.value.trim();
  if (!name || creating.value) return;
  creating.value = true;
  try {
    // 基点为当前分支时无需显式传递(等价于基于 HEAD 创建)
    const current = props.project.git?.branch;
    const startPoint =
      baseBranch.value && baseBranch.value !== current ? baseBranch.value : undefined;
    await store.checkoutBranch(props.project, name, { create: true, startPoint });
    toast.success(t("git.branch.switched", { name }));
    createOpen.value = false;
    newBranch.value = "";
  } catch (e) {
    toast.error(String(e));
  } finally {
    creating.value = false;
  }
}

// --- 当前分支:拉取 / 推送(推送被拒时给出拉取并推送的快捷修复) ---
async function pull(): Promise<boolean> {
  if (busy.value) return false;
  busy.value = "pull";
  try {
    const list = await store.pullRepository(props.project);
    if (list.length) {
      // 产生合并冲突:引导用户在 VSCode/终端中解决
      conflicts.value = list;
      conflictOpen.value = true;
      return false;
    }
    toast.success(t("git.pull.success"));
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  } finally {
    busy.value = "";
  }
}

async function push() {
  if (busy.value) return;
  busy.value = "push";
  try {
    await store.pushRepository(props.project);
    toast.success(t("git.push.success"));
  } catch (e) {
    const code = (e as Error & { code?: string }).code;
    if (code === "git_push_rejected") {
      // 远端有本地缺失的提交:不再倾倒 git 原文,给出快捷修复入口
      toast.error(t("git.push.rejected"), {
        action: { label: t("git.push.pullAndPush"), onClick: () => pullThenPush() },
      });
    } else {
      toast.error(String(e));
    }
  } finally {
    busy.value = "";
  }
}

/** 先拉取;无冲突则自动重试推送(有冲突则交给冲突引导流程) */
async function pullThenPush() {
  if (await pull()) await push();
}

// --- 跟踪更新:开启后后台定时检查远端更新并自动快进拉取(无法快进即取消,不提醒) ---
/** 主工作区下 props.project 即 store 中的对象,切换后就地更新、此处响应式跟随 */
const tracked = computed(() => props.project.auto_pull);
const trackingBusy = ref(false);

async function toggleTracking() {
  if (trackingBusy.value) return;
  trackingBusy.value = true;
  const next = !tracked.value;
  try {
    await store.setAutoPull(props.project.id, next);
    toast.success(t(next ? "git.tracking.enabled" : "git.tracking.disabled"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    trackingBusy.value = false;
  }
}

// --- 指定分支:拉取 / 推送(非当前分支由后端快进更新或直接推送,不切换工作区) ---
const branchOp = ref<{ branch: string; op: "pull" | "push" } | null>(null);

async function pullBranch(name: string): Promise<boolean> {
  if (branchOp.value) return false;
  branchOp.value = { branch: name, op: "pull" };
  try {
    const list = await store.pullRepository(props.project, name);
    if (list.length) {
      // 仅当前分支的 pull 可能产生合并冲突:引导用户在编辑器/终端中解决
      conflicts.value = list;
      conflictOpen.value = true;
      return false;
    }
    toast.success(t("git.pull.success"));
    loadBranches();
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  } finally {
    branchOp.value = null;
  }
}

async function pushBranch(name: string) {
  if (branchOp.value) return;
  branchOp.value = { branch: name, op: "push" };
  try {
    await store.pushRepository(props.project, name);
    toast.success(t("git.push.success"));
    loadBranches();
  } catch (e) {
    const code = (e as Error & { code?: string }).code;
    if (code === "git_push_rejected") {
      // 远端有本地缺失的提交:给出拉取并推送的快捷修复入口
      toast.error(t("git.push.rejected"), {
        action: { label: t("git.push.pullAndPush"), onClick: () => pullThenPushBranch(name) },
      });
    } else {
      toast.error(String(e));
    }
  } finally {
    branchOp.value = null;
  }
}

/** 先拉取;无冲突则自动重试推送 */
async function pullThenPushBranch(name: string) {
  if (await pullBranch(name)) {
    await pushBranch(name);
  }
}

// --- 删除分支:本地 -d 安全删除(未合并原地切换 -D 强删确认);远程 push --delete ---
const deleteOpen = ref(false);
const deleteTarget = ref("");
const deleteIsRemote = ref(false);
const deleteNeedsForce = ref(false);
const deleting = ref(false);

function askDeleteBranch(name: string, remote: boolean) {
  deleteTarget.value = name;
  deleteIsRemote.value = remote;
  deleteNeedsForce.value = false;
  deleteOpen.value = true;
}

async function confirmDeleteBranch() {
  const name = deleteTarget.value;
  if (!name || deleting.value) return;
  deleting.value = true;
  try {
    if (deleteIsRemote.value) {
      await store.deleteRemoteBranch(props.project, name);
      toast.success(t("git.branch.remoteDeleted", { name }));
    } else {
      await store.deleteBranch(props.project, name, deleteNeedsForce.value);
      toast.success(t("git.branch.deleted", { name }));
    }
    deleteOpen.value = false;
    loadBranches();
  } catch (e) {
    const code = (e as Error & { code?: string }).code;
    if (!deleteIsRemote.value && code === "git_branch_not_merged" && !deleteNeedsForce.value) {
      deleteNeedsForce.value = true;
    } else {
      toast.error(String(e));
      deleteOpen.value = false;
    }
  } finally {
    deleting.value = false;
  }
}

// --- worktree 模式:合回 / 变基(确认对话框与执行逻辑在 WorktreeOpsDialogs) ---
const opsRef = ref<InstanceType<typeof WorktreeOpsDialogs> | null>(null);

function openMerge() {
  const w = currentWorktree.value;
  if (w) opsRef.value?.openMerge(w);
}

function openRebase() {
  const w = currentWorktree.value;
  if (w) opsRef.value?.openRebase(w);
}

/** 合回/变基落地后:当前工作区的 ahead/behind 与变更统计已变化,刷新状态与列表 */
function onOpsChanged() {
  store.refreshGitStatus(props.project);
  loadWorktrees();
}
</script>

<template>
  <DropdownMenu v-model:open="open">
    <DropdownMenuTrigger as-child>
      <slot :op="triggerOp" />
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" class="max-h-96 w-60 overflow-y-auto">
      <!-- 当前分支操作组:提交 / Stash / 拉取 / 推送 -->
      <template v-if="git?.is_repo">
        <DropdownMenuItem
          class="gap-2 text-xs"
          :disabled="opsLocked || !hasChanges"
          :title="commitTitle"
          @click="commitOpen = true"
        >
          <GitCommitHorizontal class="h-3.5 w-3.5" />
          {{ t("git.actions.commit") }}
          <span v-if="hasChanges" class="ml-auto flex items-center gap-1 text-[10px] leading-none">
            <span v-if="staged > 0" class="flex items-center gap-0.5 font-medium text-emerald-600">
              <span class="h-1.5 w-1.5 rounded-full bg-current" />{{ staged }}
            </span>
            <span v-if="modified > 0" class="flex items-center gap-0.5 font-medium text-amber-600">
              <span class="h-1.5 w-1.5 rounded-full bg-current" />{{ modified }}
            </span>
            <span v-if="untracked > 0" class="flex items-center gap-0.5 font-medium text-sky-600">
              <span class="h-1.5 w-1.5 rounded-full bg-current" />{{ untracked }}
            </span>
          </span>
        </DropdownMenuItem>
        <DropdownMenuItem
          class="gap-2 text-xs"
          :disabled="opsLocked"
          :title="behind > 0 ? t('git.behind') : undefined"
          @click="pull"
        >
          <!-- 点击后菜单即关闭,loading 展示在外部触发按钮上(triggerOp) -->
          <ArrowDownToLine class="h-3.5 w-3.5" />
          {{ busy === "pull" ? t("git.pull.pulling") : t("git.actions.pull") }}
          <span
            v-if="behind > 0 && busy !== 'pull'"
            class="ml-auto text-[10px] font-medium leading-none text-amber-600"
            >{{ behind }}</span
          >
        </DropdownMenuItem>
        <DropdownMenuItem
          class="gap-2 text-xs"
          :disabled="opsLocked"
          :title="ahead > 0 ? t('git.ahead') : undefined"
          @click="push"
        >
          <ArrowUpToLine class="h-3.5 w-3.5" />
          {{ busy === "push" ? t("git.push.pushing") : t("git.actions.push") }}
          <span
            v-if="ahead > 0 && busy !== 'push'"
            class="ml-auto text-[10px] font-medium leading-none text-emerald-600"
            >{{ ahead }}</span
          >
        </DropdownMenuItem>
        <DropdownMenuItem class="gap-2 text-xs" :disabled="opsLocked" @click="emit('openStash')">
          <ArchiveRestore class="h-3.5 w-3.5" />
          {{ t("git.stash.manage") }}
        </DropdownMenuItem>
        <!-- 跟踪更新:按项目维度生效(worktree 视图隐藏,避免误以为只跟踪当前工作区) -->
        <DropdownMenuItem
          v-if="!currentWorktree"
          class="gap-2 text-xs"
          :disabled="opsLocked || trackingBusy"
          :title="t('git.tracking.hint')"
          @click="toggleTracking"
        >
          <Radar class="h-3.5 w-3.5" />
          {{ tracked ? t("git.tracking.stop") : t("git.tracking.action") }}
          <Check v-if="tracked" class="ml-auto h-3.5 w-3.5 text-emerald-600" />
        </DropdownMenuItem>
        <DropdownMenuSeparator />
      </template>

      <!-- worktree 模式:当前目录是 linked worktree。签出其他分支会与占用的工作区
           冲突,改为提供合回/变基;不展示新建分支与本地/远程分支列表 -->
      <template v-if="currentWorktree">
        <DropdownMenuItem
          class="gap-2 text-xs"
          :disabled="opsLocked || !currentWorktree.branch"
          @click="openMerge"
        >
          <GitMerge class="h-3.5 w-3.5" />
          {{ t("git.worktree.mergeBack") }}
        </DropdownMenuItem>
        <DropdownMenuItem
          class="gap-2 text-xs"
          :disabled="opsLocked || !currentWorktree.branch"
          :title="
            baseBehind > 0 ? t('git.worktree.rebaseUpdates', { count: baseBehind }) : undefined
          "
          @click="openRebase"
        >
          <ArrowUpToLine class="h-3.5 w-3.5" />
          {{ t("git.worktree.rebase") }}
          <span
            v-if="baseBehind > 0"
            class="ml-auto text-[10px] font-medium leading-none text-amber-600"
            >{{ baseBehind }}</span
          >
        </DropdownMenuItem>
      </template>
      <template v-else>
        <!-- 新建分支单独一组,紧随当前分支操作组 -->
        <DropdownMenuItem class="gap-2 text-xs" @click="createOpen = true">
          <GitBranchPlus class="h-3.5 w-3.5" />
          {{ t("git.branch.newBranch") }}
        </DropdownMenuItem>
        <DropdownMenuSeparator v-if="localTree.length || remoteTree.length || loading" />

        <DropdownMenuItem v-if="loading" disabled class="gap-2 text-xs">
          <Loader2 class="h-3.5 w-3.5 animate-spin" />
          {{ t("common.loading") }}
        </DropdownMenuItem>
        <template v-else>
          <template v-if="localTree.length">
            <DropdownMenuLabel class="text-xs">{{ t("git.branch.local") }}</DropdownMenuLabel>
            <GitBranchTreeItems
              :nodes="localTree"
              :current-branch="git?.branch ?? ''"
              :track-by-name="trackByName"
              :branch-op="branchOp"
              :locked="opsLocked"
              :collapsed="collapsedFolders"
              @toggle-folder="toggleFolder"
              @checkout="checkoutBranch"
              @pull="pullBranch"
              @push="pushBranch"
              @remove="askDeleteBranch"
            />
          </template>
          <template v-if="remoteTree.length">
            <DropdownMenuSeparator v-if="localTree.length" />
            <DropdownMenuLabel class="text-xs">{{ t("git.branch.remote") }}</DropdownMenuLabel>
            <GitBranchTreeItems
              :nodes="remoteTree"
              remote
              :current-branch="git?.branch ?? ''"
              :branch-op="branchOp"
              :locked="opsLocked"
              :collapsed="collapsedFolders"
              @toggle-folder="toggleFolder"
              @checkout="checkoutBranch"
              @pull="pullBranch"
              @push="pushBranch"
              @remove="askDeleteBranch"
            />
          </template>
        </template>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>

  <CommitDialog v-model:open="commitOpen" :project="project" />
  <ConflictDialog v-model:open="conflictOpen" :project="project" :conflicts="conflicts" />
  <!-- worktree 模式的合回/变基确认对话框(与 WorktreePanel 共用) -->
  <WorktreeOpsDialogs ref="opsRef" :project="project" @changed="onOpsChanged" />
  <GitBranchDeleteDialog
    v-model:open="deleteOpen"
    :branch="deleteTarget"
    :remote="deleteIsRemote"
    :needs-force="deleteNeedsForce"
    :deleting="deleting"
    @confirm="confirmDeleteBranch"
  />

  <Dialog v-model:open="createOpen">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.branch.createTitle") }}</DialogTitle>
      </DialogHeader>
      <form class="flex flex-col gap-4" @submit.prevent="createBranch">
        <Input v-model="newBranch" :placeholder="t('git.branch.createPlaceholder')" autofocus />
        <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
          {{ t("git.branch.createBaseLabel") }}
          <Select v-model="baseBranch">
            <SelectTrigger class="w-full">
              <SelectValue :placeholder="t('git.branch.createBaseLabel')" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
                <SelectItem v-for="b in branches.local" :key="b" :value="b">
                  {{ b }}
                </SelectItem>
              </SelectGroup>
              <SelectGroup v-if="remoteOnly.length">
                <SelectLabel>{{ t("git.branch.remote") }}</SelectLabel>
                <SelectItem v-for="r in remoteOnly" :key="r" :value="r">
                  {{ r }}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </label>
        <DialogFooter>
          <Button type="submit" :disabled="!newBranch.trim() || creating">
            {{ creating ? t("git.branch.creating") : t("common.create") }}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
