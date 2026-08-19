<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { ArrowUpToLine, GitBranch, GitMerge, Loader2, Trash2, Undo2 } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import type { WorktreeTreeNode } from "@/components/git/worktree-graph";

/** worktree 卡片:分支名/徽标/路径 + 合并/变基/删除操作,操作经事件交 WorktreePanel */
const { t } = useI18n();
const props = defineProps<{
  node: WorktreeTreeNode;
  /** 详情页当前选中的工作区路径,null = 主工作区 */
  activePath: string | null;
  rebasingPath: string | null;
  rebaseInterruptedPath: string | null;
  aborting: boolean;
}>();
const emit = defineEmits<{
  merge: [w: NonNullable<WorktreeTreeNode["worktree"]>];
  rebase: [w: NonNullable<WorktreeTreeNode["worktree"]>];
  abortRebase: [w: NonNullable<WorktreeTreeNode["worktree"]>];
  remove: [w: NonNullable<WorktreeTreeNode["worktree"]>];
}>();

const w = computed(() => props.node.worktree);

function emitFor(action: "merge" | "rebase" | "abortRebase" | "remove") {
  if (!w.value) {
    return;
  }
  if (action === "merge") {
    emit("merge", w.value);
  } else if (action === "rebase") {
    emit("rebase", w.value);
  } else if (action === "abortRebase") {
    emit("abortRebase", w.value);
  } else {
    emit("remove", w.value);
  }
}

/** 来源分支有该 worktree 未包含的新提交:变基可带入更新 */
const hasRebaseUpdates = computed(() => (w.value?.base_behind ?? 0) > 0);

const rebaseTitle = computed(() =>
  hasRebaseUpdates.value
    ? t("git.worktree.rebaseUpdates", { count: w.value?.base_behind })
    : t("git.worktree.rebase"),
);
</script>

<template>
  <div
    v-if="w"
    class="flex h-10 items-center gap-2 rounded-md border px-3"
    :class="activePath === w.path ? 'border-primary/60 bg-primary/5' : ''"
    :title="w.path"
  >
    <div class="flex min-w-0 flex-1 items-center gap-1.5">
      <GitBranch class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span class="truncate text-sm font-medium">{{ node.label }}</span>
      <Badge v-if="w.detached" variant="outline" class="shrink-0 text-[10px]">
        {{ t("git.worktree.detached") }}
      </Badge>
      <Badge v-if="activePath === w.path" class="shrink-0 text-[10px]">
        {{ t("git.worktree.current") }}
      </Badge>
    </div>
    <div class="flex shrink-0 items-center gap-0.5">
      <Button
        variant="ghost"
        size="xs"
        :disabled="!w.branch"
        :title="t('git.worktree.mergeBack')"
        @click="emitFor('merge')"
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
        @click="emitFor('abortRebase')"
      >
        <Undo2 class="h-3.5 w-3.5" />
      </Button>
      <Button
        v-else
        variant="ghost"
        size="xs"
        :disabled="!w.branch || rebasingPath === w.path"
        :title="rebaseTitle"
        @click="emitFor('rebase')"
      >
        <Loader2 v-if="rebasingPath === w.path" class="h-3.5 w-3.5 animate-spin" />
        <template v-else>
          <ArrowUpToLine class="h-3.5 w-3.5" :class="hasRebaseUpdates ? 'text-amber-600' : ''" />
          <span v-if="hasRebaseUpdates" class="text-[10px] font-medium leading-none text-amber-600">
            {{ w.base_behind }}
          </span>
        </template>
      </Button>
      <Button
        variant="ghost"
        size="xs"
        class="text-destructive"
        :title="t('git.worktree.remove')"
        @click="emitFor('remove')"
      >
        <Trash2 class="h-3.5 w-3.5" />
      </Button>
    </div>
  </div>
</template>
