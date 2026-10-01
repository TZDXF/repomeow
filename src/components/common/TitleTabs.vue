<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { Home, LoaderCircle, MoreHorizontal, X } from "@lucide/vue";
import { VueDraggable } from "vue-draggable-plus";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useProjectsStore } from "@/stores/projects";
import { useBackgroundTasksStore } from "@/stores/background-tasks";
import { resolveTabFromPath, useTabsStore } from "@/stores/tabs";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const tabsStore = useTabsStore();
const projectsStore = useProjectsStore();
const backgroundTasksStore = useBackgroundTasksStore();

const resolved = computed(() => resolveTabFromPath(route.path));

function projectName(id: number): string {
  return projectsStore.projects.find((p) => p.id === id)?.name ?? `#${id}`;
}

function isProjectActive(id: number): boolean {
  return resolved.value.kind === "project" && resolved.value.projectId === id;
}

/** 各项目进行中的后台任务数(前端镜像与后端任务均经 background-tasks store 汇聚) */
const runningCountByProject = computed(() => {
  const counts = new Map<number, number>();
  for (const task of backgroundTasksStore.tasks) {
    if (!task.target) continue;
    counts.set(task.target.projectId, (counts.get(task.target.projectId) ?? 0) + 1);
  }
  return counts;
});

function runningCount(id: number): number {
  return runningCountByProject.value.get(id) ?? 0;
}

function tabTitle(id: number): string {
  const name = projectName(id);
  const count = runningCount(id);
  return count > 0 ? `${name} · ${t("titleBar.tasksRunning", { count })}` : name;
}

function openHome() {
  void router.push("/");
}

function openProject(id: number) {
  void router.push(`/projects/${id}`);
}

function onCloseTab(event: MouseEvent, id: number) {
  event.stopPropagation();
  tabsStore.closeTab(id);
}

/** 悬停文本 tab 时,若名称溢出则实测溢出距离并驱动横向滚动展示全名 */
function onTabHoverEnter(event: MouseEvent) {
  const label = (event.currentTarget as HTMLElement).querySelector<HTMLElement>(
    "[data-tab-label]",
  );
  if (!label) return;
  const distance = label.scrollWidth - label.clientWidth;
  if (distance <= 0) return;
  // 匀速滚动约 40px/s,由 keyframes 前后各留停顿
  label.style.setProperty("--tab-scroll-x", `-${distance}px`);
  label.style.setProperty("--tab-scroll-duration", `${Math.max(3, distance / 40)}s`);
  label.dataset.scrolling = "true";
}

function onTabHoverLeave(event: MouseEvent) {
  const label = (event.currentTarget as HTMLElement).querySelector<HTMLElement>(
    "[data-tab-label]",
  );
  if (label) {
    delete label.dataset.scrolling;
  }
}

// ── 溢出检测:tab 总宽超出显示范围时,尾部出现「更多」按钮(点击弹出完整列表) ──
const stripRef = ref<HTMLElement | null>(null);
const trackRef = ref<HTMLElement | null>(null);
const overflow = ref(false);
let resizeObserver: ResizeObserver | undefined;

function measureOverflow() {
  const strip = stripRef.value;
  if (!strip) return;
  overflow.value = strip.scrollWidth > strip.clientWidth + 1;
}

onMounted(() => {
  resizeObserver = new ResizeObserver(measureOverflow);
  if (stripRef.value) resizeObserver.observe(stripRef.value);
  if (trackRef.value) resizeObserver.observe(trackRef.value);
  measureOverflow();
});

onBeforeUnmount(() => resizeObserver?.disconnect());

watch(
  () => tabsStore.openProjectIds.length,
  () => void nextTick(measureOverflow),
);

// ── 「更多」列表 ──
const moreOpen = ref(false);

function selectFromMore(id: number) {
  openProject(id);
  moreOpen.value = false;
}
</script>

<template>
  <!--
    标题栏内嵌的横向 tab 条:首页固定第一个,项目按打开顺序追加(名称限宽、悬停滚动展示全名、
    有后台任务时显示旋转指示),支持拖拽调整顺序;超宽时尾部出现「更多」按钮弹出完整列表。
    空白区域带 data-tauri-drag-region 可继续拖动窗口;
    交互元素 @mousedown.stop/@dblclick.stop 防止触发标题栏拖动与双击最大化。
  -->
  <div class="flex h-full min-w-0 flex-1 items-center">
    <div
      ref="stripRef"
      data-tauri-drag-region
      role="tablist"
      class="tab-strip-scroll flex h-full min-w-0 flex-1 items-center gap-1 overflow-x-auto pr-1"
    >
      <button
        type="button"
        role="tab"
        :aria-selected="resolved.kind === 'home'"
        class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :class="resolved.kind === 'home' ? 'bg-accent text-foreground' : 'bg-muted/70'"
        :title="t('titleBar.home')"
        @mousedown.stop
        @dblclick.stop
        @click="openHome"
      >
        <Home class="h-4 w-4" />
      </button>
      <div ref="trackRef" class="flex shrink-0 items-center">
        <!-- 拖拽排序走 VueDraggable(与设置页打开方式、终端页签一致);首页图标固定第一位不在列表内。
             WebView2 下原生 HTML5 拖拽的 dragover 不可靠(拖拽影像可见但收不到事件,松手不重排),
             与设置页一致用 forceFallback(pointer 事件驱动),fallback 挂到 body 避免被 tab 条 overflow 裁剪 -->
        <VueDraggable
          :model-value="tabsStore.openProjectIds"
          :animation="150"
          :force-fallback="true"
          :fallback-on-body="true"
          drag-class="opacity-40"
          ghost-class="opacity-30"
          class="flex items-center gap-1"
          @update:model-value="tabsStore.reorderTabs"
        >
          <div
            v-for="id in tabsStore.openProjectIds"
            :key="id"
            role="tab"
            :aria-selected="isProjectActive(id)"
            :tabindex="0"
            class="group flex h-7 min-w-0 max-w-40 shrink-0 cursor-pointer items-center gap-1 rounded-md pr-1 pl-2.5 text-xs font-medium transition-colors"
            :class="
              isProjectActive(id)
                ? 'bg-accent text-foreground'
                : 'bg-muted/70 text-foreground/80 hover:bg-accent hover:text-foreground'
            "
            :title="tabTitle(id)"
            @mousedown.stop
            @dblclick.stop
            @click="openProject(id)"
            @keydown.enter.prevent="openProject(id)"
            @keydown.space.prevent="openProject(id)"
            @mouseenter="onTabHoverEnter"
            @mouseleave="onTabHoverLeave"
          >
            <LoaderCircle
              v-if="runningCount(id)"
              class="h-3.5 w-3.5 shrink-0 animate-spin text-primary"
            />
            <span data-tab-label class="min-w-0 flex-1 overflow-hidden text-left whitespace-nowrap">
              <span class="inline-block whitespace-nowrap">{{ projectName(id) }}</span>
            </span>
            <!-- 关闭按钮常驻占位避免布局跳动,悬停 tab 时以透明度浮现 -->
            <span
              role="button"
              :aria-label="t('titleBar.closeTab')"
              :title="t('titleBar.closeTab')"
              class="flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground/70 opacity-0 transition-[opacity,color] group-hover:opacity-100 hover:text-destructive"
              @mousedown.stop
              @dblclick.stop
              @click.stop="onCloseTab($event, id)"
            >
              <X class="h-3 w-3" />
            </span>
          </div>
        </VueDraggable>
      </div>
    </div>

    <Popover v-if="overflow" v-model:open="moreOpen">
      <PopoverTrigger as-child>
        <button
          type="button"
          class="ml-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          :title="t('titleBar.moreTabs')"
          @mousedown.stop
          @dblclick.stop
        >
          <MoreHorizontal class="h-4 w-4" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="end" class="z-[70] w-72 p-1" @mousedown.stop>
        <button
          v-for="id in tabsStore.openProjectIds"
          :key="id"
          type="button"
          class="group flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-xs transition-colors hover:bg-accent"
          :class="isProjectActive(id) && 'bg-accent'"
          :title="tabTitle(id)"
          @click="selectFromMore(id)"
        >
          <span class="flex h-4 w-4 shrink-0 items-center justify-center">
            <LoaderCircle v-if="runningCount(id)" class="h-3.5 w-3.5 animate-spin text-primary" />
          </span>
          <span class="min-w-0 flex-1 truncate">{{ projectName(id) }}</span>
          <span
            role="button"
            :aria-label="t('titleBar.closeTab')"
            :title="t('titleBar.closeTab')"
            class="flex h-4 w-4 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
            @click.stop="onCloseTab($event, id)"
          >
            <X class="h-3 w-3" />
          </span>
        </button>
      </PopoverContent>
    </Popover>
  </div>
</template>

<style scoped>
/* 折叠 tab 条超宽时横向滚动,不显示滚动条 */
.tab-strip-scroll {
  scrollbar-width: none;
}

.tab-strip-scroll::-webkit-scrollbar {
  display: none;
}

/* 文本 tab 悬停时名称横向滚动展示全名:溢出距离与时长由 mouseenter 实测写入 CSS 变量,
   前后各留停顿,循环播放 */
[data-tab-label] > span {
  will-change: transform;
}

[data-tab-label][data-scrolling] > span {
  animation: tab-text-scroll var(--tab-scroll-duration, 6s) linear infinite;
}

@keyframes tab-text-scroll {
  0%,
  12% {
    transform: translateX(0);
  }
  88%,
  100% {
    transform: translateX(var(--tab-scroll-x, -100%));
  }
}

@media (prefers-reduced-motion: reduce) {
  [data-tab-label][data-scrolling] > span {
    animation: none;
  }
}
</style>
