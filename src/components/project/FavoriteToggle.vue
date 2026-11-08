<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Star } from "@lucide/vue";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

const { t } = useI18n(),
  props = defineProps<{ project: Project }>(),
  store = useProjectsStore();

// 未收藏时仅在父级 group hover 时显示;收藏后实心黄星常驻
async function toggle() {
  try {
    await store.setFavorite(props.project.id, !props.project.favorited_at);
  } catch (e) {
    toast.error(String(e));
  }
}
</script>

<template>
  <button
    type="button"
    class="group/star flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-all hover:bg-foreground/10 active:scale-90"
    :class="project.favorited_at ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'"
    :title="t(project.favorited_at ? 'projects.actions.unfavorite' : 'projects.actions.favorite')"
    @click.stop="toggle"
  >
    <Star
      class="h-3.5 w-3.5 transition-all group-hover/star:scale-110"
      :class="
        project.favorited_at
          ? 'fill-yellow-400 text-yellow-400'
          : 'text-muted-foreground group-hover:text-foreground group-hover/star:fill-yellow-500/30 group-hover/star:text-yellow-500'
      "
    />
  </button>
</template>
