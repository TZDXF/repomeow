<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { TriangleAlert } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import WorktreeFlowDiagram from "@/components/git/WorktreeFlowDiagram.vue";
import { useProjectsStore } from "@/stores/projects";
import type { GitBranches, GitWorktree, Project } from "@/types";

/**
 * worktree 合并/变基确认对话框,WorktreePanel(树形图行操作)与
 * GitBranchMenu(worktree 模式的分支下拉)共用。首次打开时按需加载分支与
 * worktree 列表;合并目标/变基基点默认取创建来源分支,其次主工作区当前分支。
 * rebasingPath / rebaseInterruptedPath 以 v-model 暴露,供调用方展示进行中/
 * 中断状态(如树形图上的 spinner 与中止按钮);操作落地后 emit changed 通知刷新。
 */
const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const emit = defineEmits<{ changed: [] }>();

/** 正在变基的 worktree 路径(同一时间只跑一个) */
const rebasingPath = defineModel<string | null>("rebasingPath", { default: null });
/** 变基中断(冲突待外部解决)的 worktree 路径 */
const rebaseInterruptedPath = defineModel<string | null>("rebaseInterruptedPath", {
  default: null,
});

const store = useProjectsStore();

const worktrees = ref<GitWorktree[]>([]);
const branches = ref<GitBranches>({ local: [], remote: [], tracking: [] });
/** 是否已加载过;操作落地后重置,下次打开重拉(分支/领先数已变化) */
let loaded = false;

// --- 合回确认(目标分支可选,默认创建来源分支) ---
const mergeTarget = ref<GitWorktree | null>(null);
const mergeInto = ref("");
const mergeSquash = ref(false);
const merging = ref(false);

// --- 变基确认(执行前的流程图解;onto 默认创建来源分支) ---
const rebaseTarget = ref<GitWorktree | null>(null);
const rebaseOnto = ref("");

// --- 冲突引导(复用 ConflictDialog;worktree 内冲突传 worktree 路径) ---
const conflictOpen = ref(false);
const conflictFiles = ref<string[]>([]);
const conflictPath = ref<string | undefined>(undefined);

/** 主工作区当前分支:合并/变基默认目标的兜底(调用方 project 可能是 worktree 副本) */
const mainBranch = computed(
  () => worktrees.value.find((w) => w.is_main)?.branch ?? props.project.git?.branch ?? null,
);

/** 远程分支去掉远端名前缀: "origin/team/x" -> "team/x" */
function remoteShortName(remote: string) {
  return remote.slice(remote.indexOf("/") + 1);
}

// 已被某个 worktree 检出的分支名(判断 squash 可用性)
const checkedOutBranches = computed(() => {
  return new Set(worktrees.value.map((w) => w.branch).filter((b): b is string => !!b));
});

/** 合并目标候选:除源分支外的全部本地分支 */
const mergeTargetCandidates = computed(() =>
  branches.value.local.filter((b) => b !== mergeTarget.value?.branch),
);
/** 选中的合并目标是否检出在某个 worktree(未检出时后端仅允许快进,不可 squash) */
const mergeIntoCheckedOut = computed(() => checkedOutBranches.value.has(mergeInto.value));

/** 变基 onto 候选:除自身分支外的本地 + 远程分支 */
const rebaseOntoLocal = computed(() =>
  branches.value.local.filter((b) => b !== rebaseTarget.value?.branch),
);

/** 来源分支解析为本地分支名(base 可能是 origin/x 远程引用,本地同名时映射过去) */
function resolveLocalBase(base: string | null, self: string | null): string | null {
  if (!base || base === self) return null;
  if (branches.value.local.includes(base)) return base;
  const short = remoteShortName(base);
  return branches.value.local.includes(short) ? short : null;
}

/** 合并默认目标:优先创建来源分支(本地),否则主工作区当前分支 */
function defaultMergeTarget(w: GitWorktree) {
  return resolveLocalBase(w.base_branch, w.branch) ?? mainBranch.value ?? "";
}

/** 变基默认基点:创建来源分支(本地或远程引用均可作 onto),否则主工作区当前分支 */
function defaultRebaseOnto(w: GitWorktree) {
  const base = w.base_branch;
  if (
    base &&
    base !== w.branch &&
    (branches.value.local.includes(base) || branches.value.remote.includes(base))
  ) {
    return base;
  }
  return mainBranch.value ?? "";
}

async function ensureLoaded() {
  if (loaded) return;
  const [wts, brs] = await Promise.all([
    store.listWorktrees(props.project),
    store.listBranches(props.project),
  ]);
  worktrees.value = wts;
  branches.value = brs;
  loaded = true;
}

async function openMerge(w: GitWorktree) {
  try {
    await ensureLoaded();
  } catch (e) {
    toast.error(String(e));
    return;
  }
  mergeTarget.value = w;
  mergeSquash.value = false;
  mergeInto.value = defaultMergeTarget(w);
}

async function openRebase(w: GitWorktree) {
  try {
    await ensureLoaded();
  } catch (e) {
    toast.error(String(e));
    return;
  }
  rebaseTarget.value = w;
  rebaseOnto.value = defaultRebaseOnto(w);
}

defineExpose({ openMerge, openRebase });

// 目标分支未被检出时 squash 不可用(后端无工作区可暂存)
watch(mergeInto, () => {
  if (!mergeIntoCheckedOut.value) mergeSquash.value = false;
});

function showConflicts(files: string[], path?: string) {
  conflictFiles.value = files;
  conflictPath.value = path;
  conflictOpen.value = true;
}

async function mergeBack() {
  const w = mergeTarget.value;
  const target = mergeInto.value;
  if (!w?.branch || !target || merging.value) return;
  merging.value = true;
  try {
    const { conflicts, mergedIn } = await store.mergeBranch(props.project, w.branch, {
      squash: mergeSquash.value,
      target,
    });
    if (conflicts.length) {
      // 冲突发生在目标分支所在的工作区;主工作区内合并不传 path(走主仓库语义)
      const main = worktrees.value.find((x) => x.is_main)?.path;
      showConflicts(conflicts, mergedIn && mergedIn !== main ? mergedIn : undefined);
    } else if (mergeSquash.value) {
      // squash 不自动提交:提示用户确认后走常规提交
      toast.success(t("git.worktree.squashStaged", { name: w.branch, target }));
    } else {
      toast.success(t("git.worktree.merged", { name: w.branch, target }));
    }
    loaded = false;
    mergeTarget.value = null;
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    merging.value = false;
  }
}

/** 确认对话框点"变基"后执行 */
function confirmRebase() {
  const w = rebaseTarget.value;
  rebaseTarget.value = null;
  if (w) rebase(w);
}

/** 在 worktree 内执行:将该 worktree 的分支变基到选定基点(默认创建来源分支)之上 */
async function rebase(w: GitWorktree) {
  const onto = rebaseOnto.value || mainBranch.value;
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
      toast.success(t("git.worktree.rebased", { name: w.branch ?? w.head.slice(0, 7), onto }));
    }
    loaded = false;
    emit("changed");
  } catch (e) {
    toast.error(String(e));
  } finally {
    rebasingPath.value = null;
  }
}
</script>

<template>
  <!-- 合回确认:目标分支可选(默认创建来源分支),附流程图解 -->
  <Dialog :open="!!mergeTarget" @update:open="mergeTarget = null">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>
          {{ t("git.worktree.mergeTitle", { name: mergeTarget?.branch ?? "" }) }}
        </DialogTitle>
      </DialogHeader>
      <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
        {{ t("git.worktree.mergeTargetLabel") }}
        <Select v-model="mergeInto">
          <SelectTrigger class="w-full">
            <SelectValue :placeholder="t('git.worktree.mergeTargetLabel')" />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
              <SelectItem v-for="b in mergeTargetCandidates" :key="b" :value="b">
                {{ b }}
              </SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </label>
      <p
        v-if="mergeInto && !mergeIntoCheckedOut"
        class="flex items-start gap-1.5 text-[11px] text-amber-600"
      >
        <TriangleAlert class="mt-0.5 h-3 w-3 shrink-0" />
        {{ t("git.worktree.mergeUncheckedHint") }}
      </p>
      <WorktreeFlowDiagram
        v-if="mergeTarget && mergeInto"
        kind="merge"
        :source="mergeTarget.branch ?? mergeTarget.head.slice(0, 7)"
        :target="mergeInto"
        :squash="mergeSquash"
      />
      <label class="flex items-center gap-2 text-sm">
        <Switch v-model="mergeSquash" :disabled="!mergeIntoCheckedOut" />
        {{ t("git.worktree.squash") }}
      </label>
      <DialogFooter class="gap-2">
        <Button variant="ghost" @click="mergeTarget = null">{{ t("common.cancel") }}</Button>
        <Button :disabled="merging || !mergeInto" @click="mergeBack">
          {{ merging ? t("git.worktree.merging") : t("git.worktree.mergeBack") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <!-- 变基确认:onto 默认创建来源分支,附变基前后图解 -->
  <Dialog :open="!!rebaseTarget" @update:open="rebaseTarget = null">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>
          {{ t("git.worktree.rebaseTitle", { name: rebaseTarget?.branch ?? "" }) }}
        </DialogTitle>
      </DialogHeader>
      <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
        {{ t("git.worktree.rebaseOntoLabel") }}
        <Select v-model="rebaseOnto">
          <SelectTrigger class="w-full">
            <SelectValue :placeholder="t('git.worktree.rebaseOntoLabel')" />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup v-if="rebaseOntoLocal.length">
              <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
              <SelectItem v-for="b in rebaseOntoLocal" :key="b" :value="b">
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
      <WorktreeFlowDiagram
        v-if="rebaseTarget && rebaseOnto"
        kind="rebase"
        :source="rebaseTarget.branch ?? rebaseTarget.head.slice(0, 7)"
        :target="rebaseOnto"
      />
      <p class="text-xs text-muted-foreground">
        {{ t("git.worktree.rebaseHint", { name: rebaseTarget?.branch ?? "", onto: rebaseOnto }) }}
      </p>
      <DialogFooter class="gap-2">
        <Button variant="ghost" @click="rebaseTarget = null">{{ t("common.cancel") }}</Button>
        <Button :disabled="!rebaseOnto" @click="confirmRebase">
          {{ t("git.worktree.rebase") }}
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
