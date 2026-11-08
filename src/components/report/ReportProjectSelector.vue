<script setup lang="ts">
import { DropdownMenuContent } from "@/components/common/project-floating-content";
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { Search, Tags, X } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import ScrollArea from "@/components/common/ScrollArea.vue";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import {
  filterReportProjects,
  selectVisibleProjectIds,
  type TagFilterMode,
} from "./project-selector";
import type { Project, Tag } from "@/types";

const props = withDefaults(
    defineProps<{
      projects: Project[];
      tags: Tag[];
      /**
       * true 时列表纵向撑满父容器(配合外层 class 控制高度,用于高对话框右列);
       * 默认 false 保持 max-h-36 紧凑高度(DailyReportDialog 行为不变)
       */
      fillHeight?: boolean;
      /** 多标签筛选方式;默认交集,报告历史生成入口显式使用 any 并集 */
      tagFilterMode?: TagFilterMode;
    }>(),
    { fillHeight: false, tagFilterMode: "all" },
  ),
  selectedIds = defineModel<number[]>({ required: true }),
  { t } = useI18n(),
  keyword = ref(""),
  filterTagIds = ref<number[]>([]),
  visibleProjects = computed(() =>
    filterReportProjects(props.projects, keyword.value, {
      tagIds: filterTagIds.value,
      tagFilterMode: props.tagFilterMode,
    }),
  ),
  selectedFilterTags = computed(() =>
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
  selectedIds.value = selectVisibleProjectIds(selectedIds.value, visibleProjects.value);
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
    <!-- 调用方附加控件(如定时报告的「按标签包含」),渲染在搜索/筛选行之前 -->
    <slot name="controls" />
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
    <ScrollArea
      :class="fillHeight ? 'min-h-40 flex-1 rounded-md border' : 'max-h-36 rounded-md border'"
    >
      <div class="grid grid-cols-1 gap-x-2 p-2" :class="{ 'content-start': fillHeight }">
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
    </ScrollArea>
  </div>
</template>
