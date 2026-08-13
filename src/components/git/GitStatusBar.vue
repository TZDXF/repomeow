<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { ChevronDown, FolderGit2, GitBranch, Loader2 } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import GitBranchMenu from "@/components/git/GitBranchMenu.vue";
import GitBranchTrackBadges from "@/components/git/GitBranchTrackBadges.vue";
import GitRemoteLink from "@/components/git/GitRemoteLink.vue";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();
const store = useProjectsStore();

const git = computed(() => props.project.git);
const initializing = ref(false);

const ahead = computed(() => git.value?.ahead ?? 0);
const behind = computed(() => git.value?.behind ?? 0);
/** 未提交变更总数(已暂存 + 已修改 + 未跟踪) */
const changes = computed(() => {
  const g = git.value;
  return g ? g.staged + g.modified + g.untracked : 0;
});
/** 变更标记悬浮提示:三类变更明细 */
const changesTitle = computed(
  () =>
    `${t("git.staged")} ${git.value?.staged ?? 0} · ${t("git.modified")} ${git.value?.modified ?? 0} · ${t("git.untracked")} ${git.value?.untracked ?? 0}`,
);

async function initRepo() {
  if (initializing.value) return;
  initializing.value = true;
  try {
    await store.initRepository(props.project);
    toast.success(t("git.init.success"));
  } catch (e) {
    toast.error(String(e));
  } finally {
    initializing.value = false;
  }
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5 text-xs text-muted-foreground">
    <Button
      v-if="git && !git.is_repo"
      variant="outline"
      size="xs"
      :disabled="initializing"
      @click="initRepo"
    >
      <Loader2 v-if="initializing" class="h-3.5 w-3.5 animate-spin" />
      <FolderGit2 v-else class="h-3.5 w-3.5" />
      {{ t("git.init.action") }}
    </Button>
    <template v-else-if="git">
      <GitBranchMenu :project="project">
        <!-- 拉取/推送进行中:菜单点击后已关闭,loading 展示在触发徽标上 -->
        <template #default="{ op }">
          <Badge
            variant="secondary"
            class="cursor-pointer gap-1 transition-colors hover:bg-accent"
            :title="
              op === 'pull'
                ? t('git.pull.pulling')
                : op === 'push'
                  ? t('git.push.pushing')
                  : t('git.branch.switch')
            "
          >
            <Loader2 v-if="op" class="h-3 w-3 animate-spin" />
            <GitBranch v-else class="h-3 w-3" />
            {{ git.branch ?? t("git.unknownBranch") }}
            <!-- 远端更新(领先/落后)与未提交变更标记,点开下拉可执行对应操作 -->
            <GitBranchTrackBadges :ahead="ahead" :behind="behind" />
            <span
              v-if="changes > 0"
              class="h-1.5 w-1.5 rounded-full bg-amber-500"
              :title="changesTitle"
            />
            <ChevronDown class="h-3 w-3 opacity-60" />
          </Badge>
        </template>
      </GitBranchMenu>
      <GitRemoteLink :project="project" />
      <span v-if="git.conflicted > 0" class="text-red-600">
        {{ t("git.conflicted") }}
        <span class="font-medium">{{ git.conflicted }}</span>
      </span>
    </template>
  </div>
</template>
