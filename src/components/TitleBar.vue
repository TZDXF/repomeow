<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ArrowUpCircle, Copy, FileText, Minus, Settings, Square, X } from "@lucide/vue";
import BackgroundTasksMenu from "@/components/common/BackgroundTasksMenu.vue";
import TerminalSessionsMenu from "@/components/common/TerminalSessionsMenu.vue";
import TitleTabs from "@/components/common/TitleTabs.vue";
import UpdateDialog from "@/components/update/UpdateDialog.vue";
import { useUpdateStore } from "@/stores/update";

const { t } = useI18n();
const appWindow = getCurrentWindow();
const route = useRoute();
const router = useRouter();
const isMaximized = ref(false);
const updateStore = useUpdateStore();
let unlistenResize: UnlistenFn | undefined;

// 报告/设置是标题栏右侧的固定导航图标,按当前路由高亮(资源库技能预览归属设置)
const reportsActive = computed(() => route.path.startsWith("/report-history"));
const settingsActive = computed(() => route.path.startsWith("/settings"));

/** 更新下载进度环几何(viewBox 36,半径 15.5) */
const RING_R = 15.5;
const RING_C = 2 * Math.PI * RING_R;

onMounted(async () => {
  isMaximized.value = await appWindow.isMaximized();
  unlistenResize = await appWindow.onResized(async () => {
    isMaximized.value = await appWindow.isMaximized();
  });
});

onBeforeUnmount(() => {
  unlistenResize?.();
});

function onUpdateClick() {
  // 按钮仅在检测到新版本后出现,点击打开更新详情对话框
  updateStore.dialogOpen = true;
}

function onDragRegionDblClick(event: MouseEvent) {
  // 仅在空白拖拽区域响应双击最大化，避免在按钮上双击时误触发
  if ((event.target as HTMLElement).hasAttribute("data-tauri-drag-region")) {
    appWindow.toggleMaximize();
  }
}
</script>

<template>
  <!--
    弹窗(Dialog)打开时,reka-ui 会渲染 z-50 的全屏 overlay 并把 body 设为 pointer-events: none,
    导致标题栏无法拖动。这里将标题栏提到 overlay 之上并恢复指针事件;
    @pointerdown.stop 阻止冒泡到 document,避免触发 reka-ui 的"点击外部关闭弹窗"
    (Tauri 窗口拖动监听的是 mousedown,不受影响)。
  -->
  <div
    data-tauri-drag-region
    class="pointer-events-auto relative z-[60] flex h-9 shrink-0 select-none items-center border-b bg-background pl-3"
    @dblclick="onDragRegionDblClick"
    @pointerdown.stop
  >
    <div class="flex h-full min-w-0 flex-1 items-stretch">
      <TitleTabs />
      <div data-tauri-drag-region class="h-full min-w-0 flex-1" />
    </div>
    <div class="flex h-full items-stretch">
      <BackgroundTasksMenu />
      <TerminalSessionsMenu />
      <button
        class="flex w-11 items-center justify-center transition-colors hover:bg-accent hover:text-foreground"
        :class="reportsActive ? 'bg-accent text-foreground' : 'text-muted-foreground'"
        :title="t('titleBar.reports')"
        @click="router.push('/report-history')"
      >
        <FileText class="h-4 w-4" />
      </button>
      <button
        class="flex w-11 items-center justify-center transition-colors hover:bg-accent hover:text-foreground"
        :class="settingsActive ? 'bg-accent text-foreground' : 'text-muted-foreground'"
        :title="t('titleBar.settings')"
        @click="router.push('/settings')"
      >
        <Settings class="h-4 w-4" />
      </button>
      <button
        v-if="updateStore.update"
        class="relative flex w-11 items-center justify-center text-primary transition-colors hover:bg-accent"
        :title="
          updateStore.status === 'downloading'
            ? t('titleBar.downloading', { progress: updateStore.progress })
            : t('titleBar.updateAvailable', { version: updateStore.update.version })
        "
        @click="onUpdateClick"
      >
        <!-- 下载中:环形进度条(后台下载时在此展示进度,点击可重新打开对话框) -->
        <svg
          v-if="updateStore.status === 'downloading'"
          viewBox="0 0 36 36"
          class="h-4 w-4 -rotate-90"
        >
          <circle cx="18" cy="18" :r="RING_R" fill="none" class="stroke-muted" stroke-width="4" />
          <circle
            cx="18"
            cy="18"
            :r="RING_R"
            fill="none"
            stroke-width="4"
            stroke-linecap="round"
            class="stroke-primary transition-[stroke-dashoffset] duration-200"
            :stroke-dasharray="RING_C"
            :stroke-dashoffset="RING_C * (1 - updateStore.progress / 100)"
          />
        </svg>
        <ArrowUpCircle v-else class="h-4 w-4" />
        <span
          v-if="updateStore.hasUpdate"
          class="absolute right-2.5 top-2 h-1.5 w-1.5 rounded-full bg-destructive"
        />
      </button>
      <button
        class="flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="t('titleBar.minimize')"
        @click="appWindow.minimize()"
      >
        <Minus class="h-4 w-4" />
      </button>
      <button
        class="flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :title="isMaximized ? t('titleBar.restore') : t('titleBar.maximize')"
        @click="appWindow.toggleMaximize()"
      >
        <Copy v-if="isMaximized" class="h-3.5 w-3.5" />
        <Square v-else class="h-3.5 w-3.5" />
      </button>
      <button
        class="flex w-11 items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-white"
        :title="t('titleBar.close')"
        @click="appWindow.close()"
      >
        <X class="h-4 w-4" />
      </button>
    </div>
    <UpdateDialog />
  </div>
</template>
