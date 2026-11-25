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
import { isMacOS } from "@/lib/platform";
import { resolveTitleBarLayout } from "@/lib/titlebar";
import { useUpdateStore } from "@/stores/update";

const { t } = useI18n(),
  appWindow = getCurrentWindow(),
  route = useRoute(),
  router = useRouter(),
  isMaximized = ref(false),
  updateStore = useUpdateStore();
let unlistenResize: UnlistenFn | undefined;

// macOS 走系统原生装饰(tauri.macos.conf.json):交通灯由系统画在左上角,
// 前端不再重复渲染窗口按钮,也不再自己处理双击缩放(见 lib/titlebar.ts)。
const macOS = isMacOS(),
  layout = computed(() => resolveTitleBarLayout({ macOS }));

// 报告/设置是标题栏右侧的固定导航图标,按当前路由高亮(资源库技能预览归属设置)
const reportsActive = computed(() => route.path.startsWith("/report-history")),
  settingsActive = computed(() => route.path.startsWith("/settings")),
  /** 更新下载进度环几何(viewBox 36,半径 15.5) */
  RING_R = 15.5,
  RING_C = 2 * Math.PI * RING_R;

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
  // macOS 上双击缩放交给 Tauri 内置 drag.js(它在 mouseup 触发且可被鼠标移动取消),
  // 这里再调一次 toggleMaximize 会与它互相抵消,等于双击失效。
  if (!layout.value.handleDragRegionDblClick) {
    return;
  }
  // 仅在空白拖拽区域响应双击最大化，避免在按钮上双击时误触发
  if ((event.target as HTMLElement).hasAttribute("data-tauri-drag-region")) {
    appWindow.toggleMaximize();
  }
}
</script>

<template>
  <!--
    高度取 --titlebar-height(style.css 的 :root 定义,2.25rem = 36px),
    是弹窗/浮层避让标题栏的唯一来源,不要在这里写死 h-9。
    弹窗(Dialog)打开时,reka-ui 会渲染 z-50 的全屏 overlay 并把 body 设为 pointer-events: none,
    导致标题栏无法拖动。这里将标题栏提到 overlay 之上并恢复指针事件;
    @pointerdown.stop 阻止冒泡到 document,避免触发 reka-ui 的"点击外部关闭弹窗"
    (Tauri 窗口拖动监听的是 mousedown,不受影响)。
    macOS 上窗口是系统原生装饰(Overlay 风格,webview 铺满窗口),
    左侧留出 layout.leadInsetPx 给原生交通灯;其余平台保持 pl-3。
  -->
  <div
    data-tauri-drag-region
    class="pointer-events-auto relative z-[60] flex h-[var(--titlebar-height)] shrink-0 select-none items-center border-b bg-background"
    :style="{ paddingLeft: `${layout.leadInsetPx}px` }"
    @dblclick="onDragRegionDblClick"
    @pointerdown.stop
  >
    <!-- tab 条占满左区,空白区域自身可拖拽(TitleTabs 内部带 data-tauri-drag-region) -->
    <TitleTabs />
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
      <!--
        Windows/Linux 自绘窗口按钮;macOS 由系统交通灯负责,不再渲染。
        红色关闭按钮固定在右侧是 Windows 习惯,在 macOS 上会与左上角红点冲突。
      -->
      <template v-if="layout.showWindowControls">
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
      </template>
    </div>
    <UpdateDialog />
  </div>
</template>
