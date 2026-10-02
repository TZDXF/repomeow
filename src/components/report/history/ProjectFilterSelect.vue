<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { FolderGit2, Search } from "@lucide/vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import ScrollArea from "@/components/common/ScrollArea.vue";
import { filterProjectsByKeyword } from "./project-filter";
import type { Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ projects: Project[]; checkedIds: number[] }>();
const emit = defineEmits<{ toggle: [projectId: number] }>();

const keyword = ref("");

const filtered = computed(() => filterProjectsByKeyword(props.projects, keyword.value));

/** 已选项目对象列表(下拉外展示为可点击移除的 chips) */
const selected = computed(() => props.projects.filter((p) => props.checkedIds.includes(p.id)));

// 阻止字符键冒泡触发菜单的 type-ahead,Escape 保留给菜单关闭
function onSearchKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") e.stopPropagation();
}
</script>

<template>
  <div>
    <label class="mb-1 block text-[11px] text-muted-foreground">
      {{ t("reportHistory.searchProject") }}
    </label>
    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button
          variant="outline"
          size="sm"
          class="h-7 w-full justify-start gap-1.5 px-2 text-xs font-normal"
        >
          <FolderGit2 class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span class="truncate">{{ t("reportHistory.searchProject") }}</span>
          <span
            v-if="checkedIds.length"
            class="ml-auto rounded-full bg-primary px-1.5 text-[11px] leading-4 text-primary-foreground"
            >{{ checkedIds.length }}</span
          >
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" class="w-52">
        <div class="px-1 pb-1">
          <div class="relative">
            <Search
              class="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              v-model="keyword"
              :placeholder="t('projects.home.searchPlaceholder')"
              class="h-7 rounded-md pl-7 pr-2 text-xs md:text-xs"
              @keydown="onSearchKeydown"
            />
          </div>
        </div>
        <ScrollArea class="max-h-56">
          <!-- 复选菜单项:@select.prevent 保持多选时菜单不关闭 -->
          <DropdownMenuCheckboxItem
            v-for="p in filtered"
            :key="p.id"
            :model-value="checkedIds.includes(p.id)"
            class="text-xs"
            @update:model-value="emit('toggle', p.id)"
            @select.prevent
          >
            <span class="truncate">{{ p.name }}</span>
          </DropdownMenuCheckboxItem>
          <p v-if="!projects.length" class="px-2 py-1.5 text-xs text-muted-foreground">
            {{ t("projects.home.emptyAll") }}
          </p>
          <p v-else-if="!filtered.length" class="px-2 py-1.5 text-xs text-muted-foreground">
            {{ t("projects.home.emptyFiltered") }}
          </p>
        </ScrollArea>
      </DropdownMenuContent>
    </DropdownMenu>
    <div v-if="selected.length" class="mt-1.5 flex flex-wrap gap-1">
      <span
        v-for="p in selected"
        :key="p.id"
        class="inline-flex cursor-pointer items-center gap-1 rounded-full border px-1.5 py-px text-[11px] hover:bg-accent"
        @click="emit('toggle', p.id)"
      >
        {{ p.name }}
        <span class="ml-0.5 text-muted-foreground">&times;</span>
      </span>
    </div>
  </div>
</template>
