<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import {
  ArrowDownUp,
  FileText,
  LayoutGrid,
  List,
  Plus,
  Search,
  Settings,
  Settings2,
  Tags,
  X,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import AddProjectDialog from "@/components/project/AddProjectDialog.vue";
import ProjectCard from "@/components/project/ProjectCard.vue";
import ProjectTable from "@/components/project/ProjectTable.vue";
import TagCheckList from "@/components/tags/TagCheckList.vue";
import TagManager from "@/components/tags/TagManager.vue";
import { compareFavorited } from "@/lib/favorites";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore, type ProjectsSortKey, type ProjectsViewMode } from "@/stores/settings";
import { useTagsStore } from "@/stores/tags";

const { t } = useI18n();
const store = useProjectsStore();
const tagsStore = useTagsStore();
const settings = useSettingsStore();
const router = useRouter();

// 搜索(防抖,逻辑同原 Sidebar)
const searchInput = ref(store.query);
let debounceTimer: number | undefined;
watch(searchInput, (value) => {
  window.clearTimeout(debounceTimer);
  debounceTimer = window.setTimeout(() => store.setQuery(value), 250);
});

// 视图模式与排序方式持久化到 settings store
const viewMode = computed<ProjectsViewMode>({
  get: () => settings.projectsViewMode,
  set: (v) => settings.setProjectsViewMode(v),
});
const sortKey = computed<ProjectsSortKey>({
  get: () => settings.projectsSortKey,
  set: (v) => settings.setProjectsSortKey(v),
});

const SORT_LABELS: Record<ProjectsSortKey, string> = {
  name: t("projects.home.sortByName"),
  updated: t("projects.home.sortByUpdated"),
  created: t("projects.home.sortByCreated"),
};

const selectedTags = computed(() =>
  tagsStore.tags.filter((t) => store.selectedTagIds.includes(t.id)),
);

const sortedProjects = computed(() => {
  const list = [...store.projects];
  // 收藏项目无条件置顶(组内按收藏时间倒序),其余按当前排序键排列
  switch (sortKey.value) {
    case "updated":
      // 以 git 最新提交时间衡量「最近更新」;非 git 仓库或状态未加载时回退到信息更新时间
      return list.sort(
        (a, b) =>
          compareFavorited(a, b) ||
          (b.git?.last_commit_at ?? b.updated_at) - (a.git?.last_commit_at ?? a.updated_at),
      );
    case "created":
      return list.sort((a, b) => compareFavorited(a, b) || b.created_at - a.created_at);
    default:
      return list.sort(
        (a, b) => compareFavorited(a, b) || a.name.localeCompare(b.name, "zh-Hans-CN"),
      );
  }
});
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-2 border-b px-4 py-2.5">
      <div class="relative w-64 max-w-full">
        <Search
          class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="searchInput"
          :placeholder="t('projects.home.searchPlaceholder')"
          class="h-8 pl-8 text-sm"
        />
      </div>
      <div class="flex flex-wrap items-center gap-1.5">
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button variant="outline" size="sm" class="h-8 gap-1.5">
              <Tags class="h-3.5 w-3.5" />
              {{ t("projects.home.filterTags") }}
              <span
                v-if="store.selectedTagIds.length"
                class="rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
              >
                {{ store.selectedTagIds.length }}
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" class="w-52">
            <TagCheckList
              :tags="tagsStore.tags"
              :checked-ids="store.selectedTagIds"
              @toggle="store.toggleTagFilter"
            />
            <template v-if="store.selectedTagIds.length">
              <DropdownMenuSeparator />
              <DropdownMenuItem class="gap-2 text-xs" @click="store.clearTagFilters()">
                <X class="h-3.5 w-3.5" />
                {{ t("projects.home.clearFilter") }}
              </DropdownMenuItem>
            </template>
            <DropdownMenuSeparator />
            <TagManager @refresh-projects="store.fetchProjects()">
              <Button variant="ghost" size="sm" class="h-7 w-full justify-start gap-2 px-2 text-xs">
                <Settings2 class="h-3.5 w-3.5" />
                {{ t("tags.picker.manage") }}
              </Button>
            </TagManager>
          </DropdownMenuContent>
        </DropdownMenu>
        <button
          v-for="tag in selectedTags"
          :key="tag.id"
          type="button"
          class="flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] transition-opacity hover:opacity-80"
          :style="{ backgroundColor: tag.color, borderColor: tag.color, color: '#fff' }"
          :title="t('projects.home.removeFilterTag', { name: tag.name })"
          @click="store.toggleTagFilter(tag.id)"
        >
          {{ tag.name }}
          <X class="h-2.5 w-2.5" />
        </button>
      </div>
      <div class="ml-auto flex items-center gap-2">
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button variant="outline" size="sm" class="h-8 gap-1.5">
              <ArrowDownUp class="h-3.5 w-3.5" />
              {{ SORT_LABELS[sortKey] }}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuRadioGroup v-model="sortKey">
              <DropdownMenuRadioItem value="name">{{
                t("projects.home.sortByName")
              }}</DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="updated">{{
                t("projects.home.sortByUpdated")
              }}</DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="created">{{
                t("projects.home.sortByCreated")
              }}</DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
        <div class="flex items-center rounded-md border p-0.5">
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :class="viewMode === 'grid' && 'bg-accent'"
            :title="t('projects.home.viewGrid')"
            @click="viewMode = 'grid'"
          >
            <LayoutGrid class="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :class="viewMode === 'table' && 'bg-accent'"
            :title="t('projects.home.viewTable')"
            @click="viewMode = 'table'"
          >
            <List class="h-3.5 w-3.5" />
          </Button>
        </div>
        <AddProjectDialog>
          <Button size="sm" class="h-8 gap-1.5">
            <Plus class="h-4 w-4" />
            {{ t("projects.home.addProject") }}
          </Button>
        </AddProjectDialog>
        <Button
          variant="outline"
          size="sm"
          class="h-8 gap-1.5"
          :title="t('reportHistory.title')"
          @click="router.push('/report-history')"
        >
          <FileText class="h-3.5 w-3.5" />
          {{ t("ai.entry") }}
        </Button>
        <Button
          variant="outline"
          size="icon"
          class="h-8 w-8"
          :title="t('projects.home.settings')"
          @click="router.push('/settings')"
        >
          <Settings class="h-3.5 w-3.5" />
        </Button>
      </div>
    </header>

    <div class="flex-1 overflow-y-auto">
      <div
        v-if="viewMode === 'grid'"
        class="grid gap-3 p-4 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]"
      >
        <ProjectCard v-for="p in sortedProjects" :key="p.id" :project="p" />
      </div>
      <ProjectTable v-else :projects="sortedProjects" />
      <p v-if="!sortedProjects.length" class="py-16 text-center text-sm text-muted-foreground">
        {{
          store.query || store.selectedTagIds.length
            ? t("projects.home.emptyFiltered")
            : t("projects.home.emptyAll")
        }}
      </p>
    </div>
  </div>
</template>
