import { shallowRef, watch } from "vue";
import type { RouteLocationNormalizedLoaded } from "vue-router";

export interface ProjectPage {
  projectId: number;
  route: RouteLocationNormalizedLoaded;
}

/** 仅缓存实际访问过的项目页面;关闭 tab 时销毁该项目的全部页面和草稿。 */
export function useProjectPageCache(
  currentRoute: () => RouteLocationNormalizedLoaded,
  openProjectIds: () => number[],
) {
  const pages = shallowRef<ProjectPage[]>([]);
  watch(
    [() => currentRoute().fullPath, () => [...openProjectIds()]],
    () => {
      const route = currentRoute(),
        openIds = new Set(openProjectIds()),
        next = pages.value.filter((page) => openIds.has(page.projectId)),
        match = /^\/projects\/(?<projectId>\d+)(?:\/.*)?$/.exec(route.path),
        projectId = match ? Number(match.groups!.projectId) : undefined;
      if (projectId !== undefined && openIds.has(projectId)) {
        // 不能保存全局 useRoute() 的响应式代理,否则后台页面仍会跟着当前项目变化。
        const snapshot = { ...route, params: { ...route.params }, query: { ...route.query } },
          index = next.findIndex((page) => page.route.path === route.path),
          page = { projectId, route: snapshot };
        if (index < 0) {
          next.push(page);
        } else {
          next[index] = page;
        }
      }
      pages.value = next;
    },
    { immediate: true },
  );
  return pages;
}
