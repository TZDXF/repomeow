<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { ArrowUpCircle, Copy, Minus, Square, X } from "@lucide/vue";
import BackgroundTasksMenu from "@/components/common/BackgroundTasksMenu.vue";
import UpdateDialog from "@/components/update/UpdateDialog.vue";
import { useUpdateStore } from "@/stores/update";

const { t } = useI18n();
const appWindow = getCurrentWindow();
const isMaximized = ref(false);
const updateStore = useUpdateStore();
let unlistenResize: UnlistenFn | undefined;

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
    <div
      data-tauri-drag-region
      class="flex min-w-0 flex-1 items-center gap-2 overflow-hidden text-xs font-medium text-muted-foreground"
    >
      <span class="pointer-events-none shrink-0">{{ t("app.title") }} · {{ t("app.name") }}</span>
      <BackgroundTasksMenu />
    </div>
    <div class="flex h-full items-stretch">
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
