<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { Home, X } from "@lucide/vue";
import { useProjectsStore } from "@/stores/projects";
import { resolveTabFromPath, useTabsStore } from "@/stores/tabs";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const tabsStore = useTabsStore();
const projectsStore = useProjectsStore();

// 悬停展开浮层的进出延迟:进入稍作等待避免掠过即弹,移出留缓冲允许移入浮层
const HOVER_OPEN_DELAY = 150;
const HOVER_CLOSE_DELAY = 200;

const floatOpen = ref(false);
let openTimer: ReturnType<typeof setTimeout> | undefined;
let closeTimer: ReturnType<typeof setTimeout> | undefined;

function cancelFloatTimers() {
  clearTimeout(openTimer);
  clearTimeout(closeTimer);
}

function onHoverEnter() {
  cancelFloatTimers();
  openTimer = setTimeout(() => {
    floatOpen.value = true;
  }, HOVER_OPEN_DELAY);
}

function onHoverLeave() {
  cancelFloatTimers();
  closeTimer = setTimeout(() => {
    floatOpen.value = false;
  }, HOVER_CLOSE_DELAY);
}

onBeforeUnmount(cancelFloatTimers);

const resolved = computed(() => resolveTabFromPath(route.path));

/** 头像底色按项目 id 轮换(确定性的柔和色板,亮暗主题均可读) */
const AVATAR_COLORS = [
  "bg-blue-500/15 text-blue-600 dark:text-blue-400",
  "bg-violet-500/15 text-violet-600 dark:text-violet-400",
  "bg-amber-500/15 text-amber-600 dark:text-amber-400",
  "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
  "bg-rose-500/15 text-rose-600 dark:text-rose-400",
  "bg-cyan-500/15 text-cyan-600 dark:text-cyan-400",
] as const;

function avatarClass(id: number): string {
  return AVATAR_COLORS[id % AVATAR_COLORS.length];
}

function projectName(id: number): string {
  return projectsStore.projects.find((p) => p.id === id)?.name ?? `#${id}`;
}

function initial(name: string): string {
  return (name.trim()[0] ?? "?").toUpperCase();
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
</script>

<template>
  <!--
    标题栏内嵌的横向图标 tab 条:首页固定第一个,项目按打开顺序追加。
    悬停展开浮层显示带项目名的完整列表;@mousedown.stop/@dblclick.stop 防止触发标题栏拖动与双击最大化。
  -->
  <div
    class="relative flex h-full max-w-[50%] min-w-0 items-center"
    @mouseenter="onHoverEnter"
    @mouseleave="onHoverLeave"
  >
    <div class="tab-strip-scroll flex h-full min-w-0 items-center gap-1 overflow-x-auto pr-1">
      <button
        type="button"
        class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
        :class="resolved.kind === 'home' && 'bg-accent text-foreground'"
        :title="t('titleBar.home')"
        @mousedown.stop
        @dblclick.stop
        @click="openHome"
      >
        <Home class="h-4 w-4" />
      </button>
      <div v-for="id in tabsStore.openProjectIds" :key="id" class="group relative shrink-0">
        <button
          type="button"
          class="flex h-7 w-7 items-center justify-center rounded-full text-xs font-medium transition-shadow"
          :class="[
            avatarClass(id),
            resolved.kind === 'project' && resolved.projectId === id && 'ring-2 ring-ring',
          ]"
          :title="projectName(id)"
          @mousedown.stop
          @dblclick.stop
          @click="openProject(id)"
        >
          {{ initial(projectName(id)) }}
        </button>
        <button
          type="button"
          class="absolute -right-1 -top-1 hidden h-3.5 w-3.5 items-center justify-center rounded-full border bg-background text-muted-foreground group-hover:flex hover:text-destructive"
          :title="t('titleBar.closeTab')"
          @mousedown.stop
          @dblclick.stop
          @click="onCloseTab($event, id)"
        >
          <X class="h-2.5 w-2.5" />
        </button>
      </div>
    </div>

    <!-- 悬停浮层:带项目名的完整 tab 列表,挂在标题栏根层级不会被 overflow 裁剪 -->
    <div
      v-show="floatOpen"
      class="absolute left-0 top-full z-[70] mt-1 w-72 rounded-md border bg-popover p-1.5 text-popover-foreground shadow-md"
      @mousedown.stop
      @dblclick.stop
    >
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent"
        :class="resolved.kind === 'home' && 'bg-accent'"
        @click="openHome"
      >
        <Home class="h-4 w-4 shrink-0 text-muted-foreground" />
        <span class="min-w-0 flex-1 truncate">{{ t("titleBar.home") }}</span>
      </button>
      <button
        v-for="id in tabsStore.openProjectIds"
        :key="id"
        type="button"
        class="group flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm transition-colors hover:bg-accent"
        :class="resolved.kind === 'project' && resolved.projectId === id && 'bg-accent'"
        @click="openProject(id)"
      >
        <span
          class="flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[10px] font-medium"
          :class="avatarClass(id)"
        >
          {{ initial(projectName(id)) }}
        </span>
        <span class="min-w-0 flex-1 truncate">{{ projectName(id) }}</span>
        <span
          class="flex h-5 w-5 shrink-0 items-center justify-center rounded text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100"
          :title="t('titleBar.closeTab')"
          @click.stop="onCloseTab($event, id)"
        >
          <X class="h-3.5 w-3.5" />
        </span>
      </button>
      <p
        v-if="!tabsStore.openProjectIds.length"
        class="px-2 py-1.5 text-xs text-muted-foreground"
      >
        {{ t("titleBar.noProjectTabs") }}
      </p>
    </div>
  </div>
</template>

<style scoped>
/* 折叠图标条超宽时横向滚动,不显示滚动条 */
.tab-strip-scroll {
  scrollbar-width: none;
}

.tab-strip-scroll::-webkit-scrollbar {
  display: none;
}
</style>
