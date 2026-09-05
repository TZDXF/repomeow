<script setup lang="ts">
import { computed, ref, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { Boxes, Cable, DatabaseBackup, Store } from "@lucide/vue";
import ResourceBackupTab from "./ResourceBackupTab.vue";
import ResourceMcpTab from "./ResourceMcpTab.vue";
import ResourceMarketplaceTab from "./ResourceMarketplaceTab.vue";
import ResourceSkillsTab from "./ResourceSkillsTab.vue";

const { t } = useI18n();

type ResourceTabId = "market" | "skills" | "mcp" | "backup";

const tabs: { id: ResourceTabId; icon: Component; component: Component }[] = [
  { id: "market", icon: Store, component: ResourceMarketplaceTab },
  { id: "skills", icon: Boxes, component: ResourceSkillsTab },
  { id: "mcp", icon: Cable, component: ResourceMcpTab },
  { id: "backup", icon: DatabaseBackup, component: ResourceBackupTab },
];

const activeTab = ref<ResourceTabId>("market");
const activeComponent = computed(
  () => tabs.find((tab) => tab.id === activeTab.value)?.component ?? ResourceMarketplaceTab,
);

/** 方向键在 Tab 间循环切换(简单 keyboard navigation) */
function onTablistKeydown(event: KeyboardEvent) {
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") {
    return;
  }
  const index = tabs.findIndex((tab) => tab.id === activeTab.value);
  if (index === -1) {
    return;
  }
  const delta = event.key === "ArrowRight" ? 1 : -1;
  activeTab.value = tabs[(index + delta + tabs.length) % tabs.length].id;
}
</script>

<template>
  <section>
    <h2 class="text-base font-semibold">{{ t("settings.resources.title") }}</h2>
    <p class="mt-1 text-sm text-muted-foreground">
      {{ t("settings.resources.description") }}
    </p>

    <div
      role="tablist"
      class="mt-5 inline-flex items-center gap-1 rounded-lg border bg-muted/40 p-1"
      @keydown="onTablistKeydown"
    >
      <button
        v-for="tab in tabs"
        :key="tab.id"
        type="button"
        role="tab"
        :aria-selected="activeTab === tab.id"
        :tabindex="activeTab === tab.id ? 0 : -1"
        class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors"
        :class="
          activeTab === tab.id
            ? 'bg-background text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'
        "
        @click="activeTab = tab.id"
      >
        <component :is="tab.icon" class="h-3.5 w-3.5" />
        {{ t(`settings.resources.tabs.${tab.id}`) }}
      </button>
    </div>

    <div role="tabpanel" class="mt-5">
      <KeepAlive>
        <component :is="activeComponent" />
      </KeepAlive>
    </div>
  </section>
</template>
