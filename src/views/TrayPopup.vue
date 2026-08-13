<script setup lang="ts">
// 托盘迷你项目列表窗口(类似 JetBrains Toolbox):单击托盘图标弹出,
// 头部搜索 + 精简项目行,双击行跳主窗口详情,单击展开/收起「常用命令」(无命令时单击直接打开),
// 行尾可展开「打开方式」,收藏按钮在最后。命令列表默认折叠,展开状态按项目持久化。
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { Search, TerminalSquare } from "@lucide/vue";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import OpenWithMenu from "@/components/open/OpenWithMenu.vue";
import FavoriteToggle from "@/components/project/FavoriteToggle.vue";
import TrayPinnedCommands from "@/components/project/TrayPinnedCommands.vue";
import { useCollapsibleOpen } from "@/composables/useCollapsibleOpen";
import { compareFavorited } from "@/lib/favorites";
import { cmd, onListen } from "@/lib/tauri";
import { usePinsStore } from "@/stores/pins";
import { useProjectsStore } from "@/stores/projects";
import { useSettingsStore } from "@/stores/settings";
import type { Project } from "@/types";

const { t } = useI18n();
const store = useProjectsStore();
const pinsStore = usePinsStore();
const settingsStore = useSettingsStore();
const searchInput = ref("");

// 常用命令展开状态:与详情页同一套 localStorage 持久化(scope trayPins),默认折叠
const { isOpen: isPinsOpen, setOpen: setPinsOpen } = useCollapsibleOpen("trayPins");

function pinsOf(project: Project) {
  return pinsStore.pinsOf(project.id);
}

function togglePins(project: Project) {
  const key = String(project.id);
  setPinsOpen(key, !isPinsOpen(key, false));
}

// 客户端过滤 + 按最近更新倒序;弹窗有独立 Pinia 实例,与主窗口的查询状态互不影响
const filtered = computed(() => {
  const q = searchInput.value.trim().toLowerCase();
  const list = q
    ? store.projects.filter(
        (p) =>
          p.name.toLowerCase().includes(q) ||
          p.path.toLowerCase().includes(q) ||
          p.description.toLowerCase().includes(q) ||
          p.tags.some((tag) => tag.name.toLowerCase().includes(q)),
      )
    : store.projects;
  // 收藏项目置顶(组内按收藏时间倒序),其余按最近更新倒序
  return [...list].sort((a, b) => compareFavorited(a, b) || b.updated_at - a.updated_at);
});

/** 双击项目行:显示主窗口并跳转到该项目详情页(弹窗随后因失焦自动收起) */
async function openProject(project: Project) {
  try {
    await cmd("show_main_window", { projectId: project.id });
  } catch {
    // 主窗口未就绪等情况静默失败即可
  }
}

// 单击/双击区分:单击延迟 250ms 执行,期间第二次点击(双击)取消单击动作改为打开详情
const CLICK_DELAY = 250;
let clickTimer: number | null = null;

/** 单击:有常用命令时展开/收起列表,无命令时直接打开项目 */
function onRowClick(project: Project) {
  if (clickTimer != null) {
    window.clearTimeout(clickTimer);
  }
  clickTimer = window.setTimeout(() => {
    clickTimer = null;
    if (pinsOf(project).length) {
      togglePins(project);
    } else {
      openProject(project);
    }
  }, CLICK_DELAY);
}

function onRowDblclick(project: Project) {
  if (clickTimer != null) {
    window.clearTimeout(clickTimer);
    clickTimer = null;
  }
  openProject(project);
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    cmd("hide_tray_popup").catch(() => {});
  }
}

onMounted(() => {
  // 项目列表由 App.vue 统一拉取(弹窗内 withGit: false,不拉 git 状态)
  window.addEventListener("keydown", onKeydown);
  // 每次弹窗显示时后端会发刷新事件,重新拉取以同步主窗口的数据变更
  onListen("tray-popup://refresh", () => {
    store.fetchProjects({ withGit: false });
    pinsStore.fetchPins();
    // 打开方式排序/默认项兜底重读:实时广播之外的保险(如广播注册前错过的变更)
    settingsStore.reloadOpenWith();
  });
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
  if (clickTimer != null) {
    window.clearTimeout(clickTimer);
  }
});
</script>

<template>
  <div
    data-slot="tray-popup"
    class="flex h-full flex-col overflow-hidden border bg-background shadow-2xl"
  >
    <header class="shrink-0 border-b px-3 py-2">
      <div class="relative">
        <Search class="absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="searchInput"
          :placeholder="t('trayPopup.searchPlaceholder')"
          class="h-8 pl-8 text-sm"
          autofocus
        />
      </div>
    </header>
    <ScrollArea class="min-h-0 flex-1">
      <div class="flex flex-col gap-0.5 p-2">
        <div v-for="project in filtered" :key="project.id">
          <button
            type="button"
            class="group flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent"
            @click="onRowClick(project)"
            @dblclick="onRowDblclick(project)"
          >
            <div class="min-w-0 flex-1">
              <div class="flex min-w-0 items-center gap-1.5">
                <span class="truncate text-sm font-medium">{{ project.name }}</span>
                <!-- 常用命令指示(单击行展开/收起):名称后、标签前,终端图标 + 条数 -->
                <span
                  v-if="pinsOf(project).length"
                  class="flex shrink-0 items-center gap-0.5"
                  :class="
                    isPinsOpen(String(project.id), false)
                      ? 'text-foreground'
                      : 'text-muted-foreground'
                  "
                >
                  <TerminalSquare class="h-3.5 w-3.5" />
                  <span class="text-[11px] leading-none tabular-nums">{{
                    pinsOf(project).length
                  }}</span>
                </span>
                <Badge
                  v-if="!project.path_exists"
                  variant="destructive"
                  class="shrink-0 px-1.5 py-0 text-[11px]"
                  :title="t('projects.status.pathMissingHint')"
                >
                  {{ t("projects.status.pathMissing") }}
                </Badge>
                <div
                  v-if="project.tags.length"
                  class="flex min-w-0 items-center gap-1 overflow-hidden"
                >
                  <Badge
                    v-for="tag in project.tags"
                    :key="tag.id"
                    variant="secondary"
                    class="shrink-0 px-1.5 py-0 text-[11px]"
                    :style="{ backgroundColor: tag.color + '22', color: tag.color }"
                  >
                    {{ tag.name }}
                  </Badge>
                </div>
              </div>
              <p
                v-if="project.description"
                class="mt-0.5 truncate text-xs text-muted-foreground"
                :title="project.description"
              >
                {{ project.description }}
              </p>
            </div>
            <div class="flex shrink-0 items-center">
              <div class="opacity-0 transition-opacity group-hover:opacity-100">
                <OpenWithMenu :project="project" compact />
              </div>
              <FavoriteToggle :project="project" />
            </div>
          </button>
          <!-- 被标记为「常用」的命令行内展开,点击直接执行;默认折叠,展开状态按项目持久化 -->
          <TrayPinnedCommands
            v-if="isPinsOpen(String(project.id), false)"
            :project="project"
            :pins="pinsOf(project)"
          />
        </div>
        <p v-if="!filtered.length" class="py-10 text-center text-sm text-muted-foreground">
          {{ t("trayPopup.empty") }}
        </p>
      </div>
    </ScrollArea>
  </div>
</template>
