import { useLocalStorage } from "@vueuse/core";

/**
 * 折叠/展开状态持久化:以 `${scope}:${key}` 为键记录每组是否展开,
 * 存于 localStorage,重新打开页面时恢复上次状态。
 */
const openMap = useLocalStorage<Record<string, boolean>>("repomeow.collapsible-open", {});

export function useCollapsibleOpen(scope: "scripts" | "compose" | "trayPins") {
  /** 读取展开状态;无记录时返回 fallback(如单分组默认展开) */
  function isOpen(key: string, fallback: boolean): boolean {
    return openMap.value[`${scope}:${key}`] ?? fallback;
  }

  function setOpen(key: string, open: boolean) {
    openMap.value = { ...openMap.value, [`${scope}:${key}`]: open };
  }

  return { isOpen, setOpen };
}
