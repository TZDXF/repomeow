<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { GitBranch, Radar } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import FavoriteToggle from "@/components/project/FavoriteToggle.vue";
import ProjectActionsMenu from "@/components/project/ProjectActionsMenu.vue";
import type { Project } from "@/types";

const { t } = useI18n();
defineProps<{ projects: Project[] }>();

const router = useRouter();

function open(id: number) {
  router.push(`/projects/${id}`);
}
</script>

<template>
  <table class="w-full text-sm">
    <thead>
      <tr class="border-b text-left text-xs text-muted-foreground">
        <th class="px-4 py-2 font-medium">{{ t("projects.table.name") }}</th>
        <th class="px-3 py-2 font-medium">{{ t("projects.table.path") }}</th>
        <th class="px-3 py-2 font-medium">{{ t("projects.table.branch") }}</th>
        <th class="px-3 py-2 font-medium">{{ t("projects.table.workspace") }}</th>
        <th class="px-3 py-2 font-medium">{{ t("projects.table.tags") }}</th>
        <th class="w-20 px-3 py-2 text-right font-medium">{{ t("projects.table.actions") }}</th>
      </tr>
    </thead>
    <tbody>
      <tr
        v-for="p in projects"
        :key="p.id"
        class="group cursor-pointer border-b transition-colors last:border-0 hover:bg-accent/60"
        @click="open(p.id)"
      >
        <td class="max-w-48 px-4 py-2">
          <span class="flex items-center gap-1.5 font-medium">
            <span class="min-w-0 truncate" :title="p.name">{{ p.name }}</span>
            <span v-if="p.auto_pull" class="shrink-0" :title="t('git.tracking.hint')">
              <Radar class="h-3 w-3 text-muted-foreground" />
            </span>
          </span>
          <span
            v-if="p.description"
            class="block truncate text-xs text-muted-foreground"
            :title="p.description"
          >
            {{ p.description }}
          </span>
        </td>
        <td class="max-w-64 px-3 py-2">
          <span class="flex items-center gap-1.5">
            <span class="block truncate font-mono text-xs text-muted-foreground" :title="p.path">
              {{ p.path }}
            </span>
            <Badge
              v-if="!p.path_exists"
              variant="destructive"
              class="shrink-0 px-1.5 py-0 text-[11px]"
              :title="t('projects.status.pathMissingHint')"
            >
              {{ t("projects.status.pathMissing") }}
            </Badge>
          </span>
        </td>
        <td class="px-3 py-2">
          <span v-if="p.git?.is_repo" class="flex items-center gap-1 whitespace-nowrap text-xs">
            <GitBranch class="h-3 w-3 shrink-0 text-muted-foreground" />
            <span class="max-w-28 truncate" :title="p.git.branch ?? ''">
              {{ p.git.branch ?? t("common.unknown") }}
            </span>
          </span>
          <span v-else class="text-xs text-muted-foreground">-</span>
        </td>
        <td class="px-3 py-2">
          <span v-if="p.git?.is_repo" class="flex items-center gap-2 text-xs whitespace-nowrap">
            <span v-if="p.git.staged" class="text-emerald-600">+{{ p.git.staged }}</span>
            <span v-if="p.git.modified" class="text-amber-600">~{{ p.git.modified }}</span>
            <span v-if="p.git.untracked" class="text-sky-600">?{{ p.git.untracked }}</span>
            <span
              v-if="p.git.remote_ahead"
              class="text-amber-600"
              :title="t('projects.table.remoteAhead')"
            >
              ↓{{ p.git.remote_ahead }}
            </span>
            <span
              v-if="!p.git.staged && !p.git.modified && !p.git.untracked && !p.git.remote_ahead"
              class="text-muted-foreground"
            >
              {{ t("projects.table.clean") }}
            </span>
          </span>
          <span v-else class="text-xs text-muted-foreground">-</span>
        </td>
        <td class="max-w-48 px-3 py-2">
          <div v-if="p.tags.length" class="flex flex-wrap gap-1">
            <Badge
              v-for="tag in p.tags"
              :key="tag.id"
              variant="secondary"
              class="px-1.5 py-0 text-[11px]"
              :style="{ backgroundColor: tag.color + '22', color: tag.color }"
            >
              {{ tag.name }}
            </Badge>
          </div>
        </td>
        <td class="px-3 py-1.5" @click.stop>
          <div class="flex items-center justify-end">
            <FavoriteToggle :project="p" />
            <ProjectActionsMenu :project="p" />
          </div>
        </td>
      </tr>
    </tbody>
  </table>
</template>
