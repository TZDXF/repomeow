import { computed, inject, watch, type Ref, type ComputedRef, type InjectionKey } from "vue";

/** 页面实例保留在内存中,只有当前项目的当前子页可以响应全局交互。 */
export const PROJECT_PAGE_ACTIVE: InjectionKey<ComputedRef<boolean>> =
  Symbol("project-page-active");

export function useProjectPageActive() {
  return inject(
    PROJECT_PAGE_ACTIVE,
    computed(() => true),
  );
}

/** 临时菜单不恢复,切走时关闭,使浮层及时释放全局交互锁。 */
export function watchProjectPageDeactivation(active: Readonly<Ref<boolean>>, close: () => void) {
  return watch(
    active,
    (value) => {
      if (!value) close();
    },
    { immediate: true },
  );
}

/** 缓存页面重新可见时校验业务数据;首次挂载仍由原有初始化逻辑负责。 */
export function watchProjectPageReactivation(
  active: Readonly<Ref<boolean>>,
  refresh: () => void | Promise<unknown>,
) {
  return watch(active, (value, previous) => {
    if (value && !previous) return refresh();
  });
}

export function onProjectPageReactivated(refresh: () => void | Promise<unknown>) {
  return watchProjectPageReactivation(useProjectPageActive(), refresh);
}
