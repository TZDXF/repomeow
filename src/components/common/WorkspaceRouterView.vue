<script setup lang="ts">
import { computed } from "vue";
import { RouterView, useRoute } from "vue-router";
import ProjectPageScope from "@/components/common/ProjectPageScope.vue";
import { useProjectPageCache } from "@/composables/useProjectPageCache";
import { resolveTabFromPath, useTabsStore } from "@/stores/tabs";

const route = useRoute(),
  tabs = useTabsStore(),
  isProject = computed(() => resolveTabFromPath(route.path).kind === "project"),
  pages = useProjectPageCache(
    () => route,
    () => tabs.openProjectIds,
  );
</script>

<template>
  <ProjectPageScope
    v-for="page in pages"
    :key="page.route.path"
    :route="page.route"
    :active="page.route.path === route.path"
  />
  <RouterView v-if="!isProject" />
</template>
