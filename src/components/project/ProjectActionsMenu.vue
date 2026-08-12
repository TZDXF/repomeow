<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Archive, FolderInput, FolderSync, MoreHorizontal, Star, StarOff } from "@lucide/vue";
import ConfirmDialog from "@/components/common/ConfirmDialog.vue";
import OpenWithIcon from "@/components/open/OpenWithIcon.vue";
import MoveProjectDialog from "@/components/project/MoveProjectDialog.vue";
import RelocateProjectDialog from "@/components/project/RelocateProjectDialog.vue";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { getEditorAvailability, isEditorUnavailable, sortOpenWithOptions } from "@/lib/open-with";
import type { EditorAvailability } from "@/lib/open-with";
import { cmd } from "@/lib/tauri";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { EditorKind, Project } from "@/types";

const { t } = useI18n();
const props = defineProps<{ project: Project }>();

const store = useProjectsStore();
const settings = useSettingsStore();

// 可用性探测在 lib/open-with 内模块级共享,避免每个项目实例重复 invoke
const availability = ref<EditorAvailability | null>(null);

onMounted(async () => {
  availability.value = await getEditorAvailability();
});

// 只展示已扫描到的编辑器;探测中途(null)不过滤,避免闪烁。顺序遵循设置页拖拽结果
const visibleOptions = computed(() =>
  sortOpenWithOptions(settings.openWithOrder).filter(
    (opt) => !isEditorUnavailable(opt.kind, availability.value),
  ),
);

async function openWith(kind: EditorKind) {
  try {
    await cmd("open_with", { path: props.project.path, kind });
  } catch (e) {
    toast.error(String(e));
  }
}

const archiveConfirmOpen = ref(false);
const relocateOpen = ref(false);
const moveOpen = ref(false);

async function archive() {
  try {
    await store.archiveProject(props.project.id);
    toast.success(t("projects.actions.archiveSuccess", { name: props.project.name }));
  } catch (e) {
    toast.error(String(e));
  }
}

async function toggleFavorite() {
  const favorite = !props.project.favorited_at;
  try {
    await store.setFavorite(props.project.id, favorite);
    toast.success(
      t(favorite ? "projects.actions.favoriteSuccess" : "projects.actions.unfavoriteSuccess", {
        name: props.project.name,
      }),
    );
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button
        variant="ghost"
        size="icon"
        class="h-7 w-7"
        :title="t('projects.actions.more')"
        @click.stop
      >
        <MoreHorizontal class="h-3.5 w-3.5" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-44" @click.stop>
      <DropdownMenuItem
        v-for="opt in visibleOptions"
        :key="opt.kind"
        class="gap-2 text-xs"
        @click="openWith(opt.kind)"
      >
        <OpenWithIcon :kind="opt.kind" :icon="opt.icon" icon-class="h-3.5 w-3.5" />
        {{ t(opt.descKey) }}
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem class="gap-2 text-xs" @click="toggleFavorite">
        <StarOff v-if="project.favorited_at" class="h-3.5 w-3.5" />
        <Star v-else class="h-3.5 w-3.5" />
        {{ t(project.favorited_at ? "projects.actions.unfavorite" : "projects.actions.favorite") }}
      </DropdownMenuItem>
      <DropdownMenuItem v-if="project.path_exists" class="gap-2 text-xs" @click="moveOpen = true">
        <FolderInput class="h-3.5 w-3.5" />
        {{ t("projects.actions.moveDir") }}
      </DropdownMenuItem>
      <DropdownMenuItem class="gap-2 text-xs" @click="relocateOpen = true">
        <FolderSync class="h-3.5 w-3.5" />
        {{ t("projects.actions.relocate") }}
      </DropdownMenuItem>
      <DropdownMenuItem
        variant="destructive"
        class="gap-2 text-xs"
        @click="archiveConfirmOpen = true"
      >
        <Archive class="h-3.5 w-3.5" />
        {{ t("projects.actions.archive") }}
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
  <ConfirmDialog
    v-model:open="archiveConfirmOpen"
    :title="t('projects.actions.archive')"
    :description="t('projects.actions.archiveConfirm', { name: project.name })"
    @confirm="archive"
  />
  <RelocateProjectDialog v-model:open="relocateOpen" :project="project" />
  <MoveProjectDialog v-model:open="moveOpen" :project="project" />
</template>
