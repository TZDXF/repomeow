<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { ArrowDownToLine, ArrowUpToLine, GitCommitHorizontal, Loader2 } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import CommitDialog from "@/components/git/CommitDialog.vue";
import ConflictDialog from "@/components/git/ConflictDialog.vue";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

type Op = "pull" | "push" | "";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const store = useProjectsStore();

const git = computed(() => props.project.git);
const busy = ref<Op>("");

const commitOpen = ref(false);
const conflictOpen = ref(false);
const conflicts = ref<string[]>([]);

const hasChanges = computed(
  () => !!git.value && git.value.staged + git.value.modified + git.value.untracked > 0,
);

const staged = computed(() => git.value?.staged ?? 0);
const modified = computed(() => git.value?.modified ?? 0);
const untracked = computed(() => git.value?.untracked ?? 0);
const ahead = computed(() => git.value?.ahead ?? 0);
const behind = computed(() => git.value?.behind ?? 0);

/** 提交按钮悬浮提示:三类变更明细 */
const commitTitle = computed(
  () =>
    `${t("git.staged")} ${staged.value} · ${t("git.modified")} ${modified.value} · ${t("git.untracked")} ${untracked.value}`,
);

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
</script>

<template>
  <div v-if="git?.is_repo" class="flex items-center gap-1.5">
    <Button
      variant="outline"
      size="xs"
      :disabled="busy !== '' || !hasChanges"
      :title="commitTitle"
      @click="commitOpen = true"
    >
      <GitCommitHorizontal class="h-3.5 w-3.5" />
      {{ t("git.actions.commit") }}
      <span v-if="hasChanges" class="ml-0.5 flex items-center gap-1 text-[10px] leading-none">
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
    </Button>
    <Button
      variant="outline"
      size="xs"
      :disabled="busy !== ''"
      :title="behind > 0 ? t('git.behind') : undefined"
      @click="pull"
    >
      <Loader2 v-if="busy === 'pull'" class="h-3.5 w-3.5 animate-spin" />
      <ArrowDownToLine v-else class="h-3.5 w-3.5" />
      {{ busy === "pull" ? t("git.pull.pulling") : t("git.actions.pull") }}
      <span
        v-if="behind > 0 && busy !== 'pull'"
        class="ml-0.5 text-[10px] font-medium leading-none text-amber-600"
        >{{ behind }}</span
      >
    </Button>
    <Button
      variant="outline"
      size="xs"
      :disabled="busy !== ''"
      :title="ahead > 0 ? t('git.ahead') : undefined"
      @click="push"
    >
      <Loader2 v-if="busy === 'push'" class="h-3.5 w-3.5 animate-spin" />
      <ArrowUpToLine v-else class="h-3.5 w-3.5" />
      {{ busy === "push" ? t("git.push.pushing") : t("git.actions.push") }}
      <span
        v-if="ahead > 0 && busy !== 'push'"
        class="ml-0.5 text-[10px] font-medium leading-none text-emerald-600"
        >{{ ahead }}</span
      >
    </Button>

    <CommitDialog v-model:open="commitOpen" :project="project" />
    <ConflictDialog v-model:open="conflictOpen" :project="project" :conflicts="conflicts" />
  </div>
</template>
