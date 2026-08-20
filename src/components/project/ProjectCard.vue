<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { GitBranch, Radar } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import FavoriteToggle from "@/components/project/FavoriteToggle.vue";
import ProjectActionsMenu from "@/components/project/ProjectActionsMenu.vue";
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();

const router = useRouter();

function open() {
  router.push(`/projects/${props.project.id}`);
}
</script>

<template>
  <div
    class="group cursor-pointer rounded-md border px-3 py-2 transition-colors hover:bg-accent/60"
    @click="open"
  >
    <div class="flex items-center justify-between gap-2">
      <span class="flex min-w-0 items-center gap-1.5">
        <span class="truncate text-sm font-medium">{{ project.name }}</span>
        <span v-if="project.auto_pull" class="shrink-0" :title="t('git.tracking.hint')">
          <Radar class="h-3.5 w-3.5 text-muted-foreground" />
        </span>
        <Badge
          v-if="!project.path_exists"
          variant="destructive"
          class="shrink-0 px-1.5 py-0 text-[11px]"
          :title="t('projects.status.pathMissingHint')"
        >
          {{ t("projects.status.pathMissing") }}
        </Badge>
      </span>
      <div class="flex shrink-0 items-center">
        <FavoriteToggle :project="project" />
        <div class="flex items-center opacity-0 transition-opacity group-hover:opacity-100">
          <ProjectActionsMenu :project="project" />
        </div>
      </div>
    </div>
    <p v-if="project.description" class="mt-0.5 truncate text-xs" :title="project.description">
      {{ project.description }}
    </p>
    <p class="truncate text-xs text-muted-foreground" :title="project.path">
      {{ project.path }}
    </p>
    <div
      v-if="project.git?.is_repo"
      class="mt-1 flex items-center gap-2 text-[11px] text-muted-foreground"
    >
      <span class="flex min-w-0 items-center gap-1">
        <GitBranch class="h-3 w-3 shrink-0" />
        <span class="truncate">{{ project.git.branch ?? t("common.unknown") }}</span>
      </span>
      <span v-if="project.git.staged" class="text-emerald-600"> +{{ project.git.staged }} </span>
      <span v-if="project.git.modified" class="text-amber-600"> ~{{ project.git.modified }} </span>
      <span v-if="project.git.untracked" class="text-sky-600"> ?{{ project.git.untracked }} </span>
      <span
        v-if="project.git.remote_ahead"
        class="text-amber-600"
        :title="t('projects.card.remoteAhead')"
      >
        ↓{{ project.git.remote_ahead }}
      </span>
    </div>
    <div v-if="project.tags.length" class="mt-1.5 flex flex-wrap gap-1">
      <Badge
        v-for="tag in project.tags"
        :key="tag.id"
        variant="secondary"
        class="px-1.5 py-0 text-[11px]"
        :style="{ backgroundColor: tag.color + '22', color: tag.color }"
      >
        {{ tag.name }}
      </Badge>
    </div>
  </div>
</template>
