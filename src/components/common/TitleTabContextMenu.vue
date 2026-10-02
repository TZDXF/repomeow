<script setup lang="ts">
import { useI18n } from "vue-i18n";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { useTabsStore } from "@/stores/tabs";

const props = defineProps<{ projectId: number }>();
const { t } = useI18n();
const tabsStore = useTabsStore();
</script>

<template>
  <ContextMenu :modal="false">
    <ContextMenuTrigger as-child>
      <slot />
    </ContextMenuTrigger>
    <!-- 标题栏为 z-60,菜单需在其上方,并阻止鼠标事件触发窗口拖动。 -->
    <ContextMenuContent class="z-[70] w-40" @mousedown.stop @pointerdown.stop>
      <ContextMenuItem
        :disabled="tabsStore.openProjectIds.length <= 1"
        @select="tabsStore.closeOtherTabs(props.projectId)"
      >
        {{ t("titleBar.closeOtherTabs") }}
      </ContextMenuItem>
      <ContextMenuItem @select="tabsStore.closeAllTabs()">
        {{ t("titleBar.closeAllTabs") }}
      </ContextMenuItem>
      <ContextMenuItem
        :disabled="
          tabsStore.openProjectIds.indexOf(props.projectId) === tabsStore.openProjectIds.length - 1
        "
        @select="tabsStore.closeTabsToRight(props.projectId)"
      >
        {{ t("titleBar.closeTabsToRight") }}
      </ContextMenuItem>
    </ContextMenuContent>
  </ContextMenu>
</template>
