<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
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
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import { useProjectsStore } from "@/stores/projects";
import type { GitBranches, Project } from "@/types";

type BranchOp = "merge" | "squash" | "rebase";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { required: true });
/** 由父级在打开前预设操作入口("merge" 或 "rebase"),对话框内仍可切换 */
const initialOp = defineModel<"merge" | "rebase">("initialOp", { default: "merge" });

const store = useProjectsStore();

const branches = ref<GitBranches>({ local: [], remote: [], tracking: [] });
const source = ref("");
const op = ref<BranchOp>("merge");
const running = ref(false);

// 冲突引导(合并/变基冲突都发生在项目主工作区)
const conflictOpen = ref(false);
const conflictFiles = ref<string[]>([]);

const currentBranch = computed(() => props.project.git?.branch ?? null);

/** 远程分支去掉远端名前缀: "origin/team/x" -> "team/x" */
function remoteShortName(remote: string) {
  return remote.slice(remote.indexOf("/") + 1);
}

// 已有本地同名分支的远程分支不重复展示(与 GitBranchMenu 一致)
const remoteOnly = computed(() => {
  const local = new Set(branches.value.local);
  return branches.value.remote.filter((r) => !local.has(remoteShortName(r)));
});

// 源分支不含当前分支(合并/变基自身无意义)
const localSources = computed(() => branches.value.local.filter((b) => b !== currentBranch.value));

watch(open, async (v) => {
  if (!v) return;
  op.value = initialOp.value;
  source.value = "";
  try {
    branches.value = await store.listBranches(props.project);
  } catch (e) {
    toast.error(String(e));
  }
});

async function run() {
  const branch = source.value;
  if (!branch || running.value) return;
  running.value = true;
  try {
    if (op.value === "rebase") {
      const result = await store.rebaseBranch(props.project, branch);
      if (result.inProgress) {
        if (result.conflicts.length) {
          conflictFiles.value = result.conflicts;
          conflictOpen.value = true;
        }
        toast.warning(t("git.branchOps.rebaseInterrupted"));
      } else {
        toast.success(t("git.branchOps.rebased", { name: branch }));
        open.value = false;
      }
    } else {
      const conflicts = await store.mergeBranch(props.project, branch, {
        squash: op.value === "squash",
      });
      if (conflicts.length) {
        conflictFiles.value = conflicts;
        conflictOpen.value = true;
      } else {
        toast.success(
          t(op.value === "squash" ? "git.branchOps.squashStaged" : "git.branchOps.merged", {
            name: branch,
          }),
        );
        open.value = false;
      }
    }
  } catch (e) {
    toast.error(String(e));
  } finally {
    running.value = false;
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ t("git.branchOps.title") }}</DialogTitle>
      </DialogHeader>
      <div class="flex flex-col gap-4">
        <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
          {{ t("git.branchOps.sourceLabel") }}
          <Select v-model="source">
            <SelectTrigger class="w-full">
              <SelectValue :placeholder="t('git.branchOps.sourceLabel')" />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectLabel>{{ t("git.branch.local") }}</SelectLabel>
                <SelectItem v-for="b in localSources" :key="b" :value="b">
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
        <label class="flex flex-col gap-1.5 text-xs text-muted-foreground">
          {{ t("git.branchOps.actionLabel") }}
          <Select v-model="op">
            <SelectTrigger class="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="merge">{{ t("git.branchOps.opMerge") }}</SelectItem>
              <SelectItem value="squash">{{ t("git.branchOps.opSquash") }}</SelectItem>
              <SelectItem value="rebase">{{ t("git.branchOps.opRebase") }}</SelectItem>
            </SelectContent>
          </Select>
        </label>
        <p class="text-xs text-muted-foreground">{{ t(`git.branchOps.hint_${op}`) }}</p>
      </div>
      <DialogFooter class="gap-2">
        <Button variant="ghost" @click="open = false">{{ t("common.cancel") }}</Button>
        <Button :disabled="!source || running" @click="run">
          {{ running ? t("git.branchOps.running") : t("git.branchOps.run") }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <ConflictDialog v-model:open="conflictOpen" :project="project" :conflicts="conflictFiles" />
</template>
