import { ref, watch } from "vue";
import { defineStore } from "pinia";
import { router } from "@/router";

export type TabKind = "home" | "project" | "history" | "settings" | "other";

export interface ResolvedTab {
  kind: TabKind;
  projectId?: number;
}

/** 从路由 path 解析所属 tab:首页 / 项目(含文件、Wiki、提交图子页)/ 报告 / 设置,其余(托盘弹窗等)不归属任何 tab */
export function resolveTabFromPath(path: string): ResolvedTab {
  if (path === "/") {
    return { kind: "home" };
  }
  const projectMatch = /^\/projects\/(\d+)(?:\/.*)?$/.exec(path);
  if (projectMatch) {
    return { kind: "project", projectId: Number(projectMatch[1]) };
  }
  if (path === "/report-history" || path.startsWith("/report-history/")) {
    return { kind: "history" };
  }
  if (path === "/settings" || path.startsWith("/settings/")) {
    return { kind: "settings" };
  }
  return { kind: "other" };
}

const STORAGE_KEY = "repomeow.tabs.v1";

function loadStoredProjectIds(): number[] {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is number => typeof id === "number" && Number.isFinite(id));
  } catch {
    return [];
  }
}

/**
 * 标题栏内嵌项目 tab 状态:打开过的项目各占一个 tab(按打开顺序排列),持久化到 localStorage。
 * 路由进入项目页时自动建档 —— 卡片点击、托盘跳转、后台任务、终端会话菜单等入口无需单独接线。
 */
export const useTabsStore = defineStore("title-tabs", () => {
  const openProjectIds = ref<number[]>(loadStoredProjectIds());

  function persist() {
    try {
      globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(openProjectIds.value));
    } catch {
      /* localStorage 不可用时静默降级,tab 仅存内存 */
    }
  }

  function openProject(id: number) {
    if (!Number.isFinite(id) || openProjectIds.value.includes(id)) return;
    openProjectIds.value.push(id);
    persist();
  }

  /** 批量移除 tab,仅持久化和导航一次,避免中间路由重新打开已关闭的 tab。 */
  function closeTabs(ids: number[]): ResolvedTab | null {
    const closingIds = new Set(ids);
    const previousIds = openProjectIds.value;
    const remainingIds = previousIds.filter((id) => !closingIds.has(id));
    if (remainingIds.length === previousIds.length) return null;
    openProjectIds.value = remainingIds;
    persist();

    const active = resolveTabFromPath(router.currentRoute.value.path);
    if (
      active.kind !== "project" ||
      active.projectId === undefined ||
      !closingIds.has(active.projectId)
    ) {
      return null;
    }
    const index = previousIds.indexOf(active.projectId);
    // 从原激活位置向右寻找保留的 tab,没有则向左寻找。
    const nextId =
      previousIds.slice(index + 1).find((id) => !closingIds.has(id)) ??
      previousIds
        .slice(0, index)
        .reverse()
        .find((id) => !closingIds.has(id));
    if (nextId === undefined) {
      void router.push("/");
      return { kind: "home" };
    }
    void router.push(`/projects/${nextId}`);
    return { kind: "project", projectId: nextId };
  }

  /** 关闭激活 tab 时切到相邻项目(优先右侧),后台 tab 关闭时保留当前页面。 */
  function closeTab(id: number): ResolvedTab | null {
    return closeTabs([id]);
  }

  function closeOtherTabs(id: number): ResolvedTab | null {
    if (!openProjectIds.value.includes(id)) return null;
    return closeTabs(openProjectIds.value.filter((projectId) => projectId !== id));
  }

  function closeAllTabs(): ResolvedTab | null {
    return closeTabs(openProjectIds.value);
  }

  /** 以当前显示顺序为准,首页不在可关闭列表内。 */
  function closeTabsToRight(id: number): ResolvedTab | null {
    const index = openProjectIds.value.indexOf(id);
    if (index === -1) return null;
    return closeTabs(openProjectIds.value.slice(index + 1));
  }

  /**
   * 拖拽排序:VueDraggable 拖拽完成后整体回写顺序并持久化。
   * 首页 tab 固定第一位,不在回写列表内。
   */
  function reorderTabs(ids: number[]) {
    openProjectIds.value = [...ids];
    persist();
  }

  watch(
    () => router.currentRoute.value.path,
    (path) => {
      const resolved = resolveTabFromPath(path);
      if (resolved.kind === "project" && resolved.projectId !== undefined) {
        openProject(resolved.projectId);
      }
    },
    { immediate: true },
  );

  return {
    openProjectIds,
    openProject,
    closeTab,
    closeOtherTabs,
    closeAllTabs,
    closeTabsToRight,
    reorderTabs,
  };
});
