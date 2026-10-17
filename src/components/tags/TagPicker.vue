<script setup lang="ts">
import { DropdownMenuContent } from "@/components/common/project-floating-content";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Settings2, Tags, X } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import TagManager from "@/components/tags/TagManager.vue";
import { useProjectsStore } from "@/stores/projects";
import { useTagsStore } from "@/stores/tags";
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();

const tagsStore = useTagsStore();
const projectsStore = useProjectsStore();

function hasTag(tagId: number) {
  return props.project.tags.some((t) => t.id === tagId);
}

async function apply(tagIds: number[]) {
  try {
    await tagsStore.setProjectTags(props.project.id, tagIds);
    await projectsStore.refreshProject(props.project.id);
  } catch (e) {
    toast.error(String(e));
  }
}

async function toggleTag(tagId: number) {
  const next = hasTag(tagId)
    ? props.project.tags.filter((t) => t.id !== tagId).map((t) => t.id)
    : [...props.project.tags.map((t) => t.id), tagId];
  await apply(next);
}

async function removeTag(tagId: number) {
  await apply(props.project.tags.filter((t) => t.id !== tagId).map((t) => t.id));
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-1.5">
    <Badge
      v-for="tag in project.tags"
      :key="tag.id"
      variant="secondary"
      class="gap-1 pr-1"
      :style="{ backgroundColor: tag.color + '22', color: tag.color }"
    >
      {{ tag.name }}
      <button
        type="button"
        class="rounded-full p-0.5 hover:bg-black/10"
        :title="t('tags.picker.remove')"
        @click="removeTag(tag.id)"
      >
        <X class="h-3 w-3" />
      </button>
    </Badge>
    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button variant="outline" size="sm" class="h-6 gap-1 px-2 text-xs">
          <Tags class="h-3 w-3" />
          {{ t("tags.picker.add") }}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="w-52">
        <TagCheckList
          :tags="tagsStore.tags"
          :checked-ids="project.tags.map((t) => t.id)"
          @toggle="toggleTag"
        />
        <DropdownMenuSeparator />
        <TagManager @refresh-projects="projectsStore.fetchProjects()">
          <Button variant="ghost" size="sm" class="h-7 w-full justify-start gap-2 px-2 text-xs">
            <Settings2 class="h-3.5 w-3.5" />
            {{ t("tags.picker.manage") }}
          </Button>
        </TagManager>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
