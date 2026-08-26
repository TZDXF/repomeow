<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { Search, Tags, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import type { Project, Tag } from "@/types";

const props = defineProps<{ projects: Project[]; tags: Tag[] }>();
const selectedIds = defineModel<number[]>({ required: true });
const { t } = useI18n();

const keyword = ref("");
const filterTagIds = ref<number[]>([]);
const visibleProjects = computed(() => {
  const query = keyword.value.trim().toLowerCase();
  return props.projects.filter((project) => {
    const matchesQuery =
      !query ||
      project.name.toLowerCase().includes(query) ||
      project.path.toLowerCase().includes(query);
    const matchesTags =
      !filterTagIds.value.length ||
      filterTagIds.value.every((id) => project.tags.some((tag) => tag.id === id));
    return matchesQuery && matchesTags;
  });
});
const selectedFilterTags = computed(() =>
  props.tags.filter((tag) => filterTagIds.value.includes(tag.id)),
);

function toggleTagFilter(id: number) {
  filterTagIds.value = filterTagIds.value.includes(id)
    ? filterTagIds.value.filter((value) => value !== id)
    : [...filterTagIds.value, id];
}

function toggleProject(id: number) {
  selectedIds.value = selectedIds.value.includes(id)
    ? selectedIds.value.filter((value) => value !== id)
    : [...selectedIds.value, id];
}

function selectVisible() {
  selectedIds.value = [
    ...new Set([...selectedIds.value, ...visibleProjects.value.map((p) => p.id)]),
  ];
}
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div class="flex items-center justify-between">
      <label class="text-sm font-medium">{{ t("report.selectProjects") }}</label>
      <div class="flex gap-1">
        <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" @click="selectVisible">
          {{ t("report.selectAll") }}
        </Button>
        <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" @click="selectedIds = []">
          {{ t("report.clear") }}
        </Button>
      </div>
    </div>
    <div class="flex items-center gap-1.5">
      <div class="relative flex-1">
        <Search
          class="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="keyword"
          :placeholder="t('report.projectSearchPlaceholder')"
          class="h-7 pl-7 text-xs"
        />
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger as-child>
          <Button variant="outline" size="sm" class="h-7 gap-1.5 px-2 text-xs">
            <Tags class="h-3.5 w-3.5" />
            {{ t("projects.home.filterTags") }}
            <span
              v-if="filterTagIds.length"
              class="rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
            >
              {{ filterTagIds.length }}
            </span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-52">
          <TagCheckList :tags="tags" :checked-ids="filterTagIds" @toggle="toggleTagFilter" />
          <template v-if="filterTagIds.length">
            <DropdownMenuSeparator />
            <DropdownMenuItem class="gap-2 text-xs" @click="filterTagIds = []">
              <X class="h-3.5 w-3.5" />
              {{ t("projects.home.clearFilter") }}
            </DropdownMenuItem>
          </template>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
    <div v-if="selectedFilterTags.length" class="flex flex-wrap items-center gap-1.5">
      <button
        v-for="tag in selectedFilterTags"
        :key="tag.id"
        type="button"
        class="flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-opacity hover:opacity-80"
        :style="{ backgroundColor: tag.color, borderColor: tag.color, color: '#fff' }"
        :title="t('projects.home.removeFilterTag', { name: tag.name })"
        @click="toggleTagFilter(tag.id)"
      >
        {{ tag.name }}
        <X class="h-2.5 w-2.5" />
      </button>
    </div>
    <div class="grid max-h-36 grid-cols-1 gap-x-2 overflow-y-auto rounded-md border p-2">
      <label
        v-for="project in visibleProjects"
        :key="project.id"
        class="flex cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-sm hover:bg-accent"
      >
        <input
          type="checkbox"
          class="h-3.5 w-3.5 shrink-0 accent-primary"
          :checked="selectedIds.includes(project.id)"
          @change="toggleProject(project.id)"
        />
        <span class="truncate" :title="project.path">{{ project.name }}</span>
      </label>
      <p v-if="!visibleProjects.length" class="px-1.5 py-2 text-xs text-muted-foreground">
        {{ t("report.noMatch") }}
      </p>
    </div>
  </div>
</template>
