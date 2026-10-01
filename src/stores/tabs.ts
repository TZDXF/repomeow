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

  /**
   * 关闭项目 tab。关闭的是当前激活 tab 时导航到相邻项目 tab(优先右侧),没有项目 tab 了则回首页;
   * 关闭的是后台 tab 则停留原页面。返回导航去向(单测断言用)。
   */
  function closeTab(id: number): ResolvedTab | null {
    const index = openProjectIds.value.indexOf(id);
    if (index === -1) return null;
    openProjectIds.value.splice(index, 1);
    persist();
    const active = resolveTabFromPath(router.currentRoute.value.path);
    if (active.kind !== "project" || active.projectId !== id) {
      return null;
    }
    const nextId = openProjectIds.value[Math.min(index, openProjectIds.value.length - 1)];
    if (nextId === undefined) {
      void router.push("/");
      return { kind: "home" };
    }
    void router.push(`/projects/${nextId}`);
    return { kind: "project", projectId: nextId };
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

  return { openProjectIds, openProject, closeTab };
});
