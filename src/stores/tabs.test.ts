import { beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { nextTick } from "vue";
import { createPinia, setActivePinia } from "pinia";
import { resolveTabFromPath, useTabsStore } from "@/stores/tabs";
import { router } from "@/router";

// 用带响应式的假 router 替代真实 hash 路由,测试内直接改 currentRoute 驱动 store 的自动建档
vi.mock("@/router", async () => {
  const { ref } = await import("vue");
  const currentRoute = ref<{ path: string }>({ path: "/" });
  return {
    router: {
      currentRoute,
      push: vi.fn(),
    },
  };
});

const pushMock = router.push as unknown as Mock;
const routeRef = router.currentRoute as unknown as { value: { path: string } };

function setRoute(path: string) {
  routeRef.value = { path };
  return nextTick();
}

describe("resolveTabFromPath", () => {
  it("首页与项目路由(含文件/Wiki/提交图子页)归属对应 tab", () => {
    expect(resolveTabFromPath("/")).toEqual({ kind: "home" });
    expect(resolveTabFromPath("/projects/3")).toEqual({ kind: "project", projectId: 3 });
    expect(resolveTabFromPath("/projects/12/files")).toEqual({ kind: "project", projectId: 12 });
    expect(resolveTabFromPath("/projects/12/wiki")).toEqual({ kind: "project", projectId: 12 });
    expect(resolveTabFromPath("/projects/12/graph")).toEqual({ kind: "project", projectId: 12 });
  });

  it("报告与设置路由(含资源库技能预览子页)归属对应 tab", () => {
    expect(resolveTabFromPath("/report-history")).toEqual({ kind: "history" });
    expect(resolveTabFromPath("/report-history/")).toEqual({ kind: "history" });
    expect(resolveTabFromPath("/settings")).toEqual({ kind: "settings" });
    expect(resolveTabFromPath("/settings/resources/skills/9")).toEqual({ kind: "settings" });
  });

  it("非法项目 id 与托盘路由不归属任何 tab", () => {
    expect(resolveTabFromPath("/projects/abc")).toEqual({ kind: "other" });
    expect(resolveTabFromPath("/projects/")).toEqual({ kind: "other" });
    expect(resolveTabFromPath("/tray")).toEqual({ kind: "other" });
  });
});

describe("tabs store", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    delete (globalThis as { localStorage?: unknown }).localStorage;
    setActivePinia(createPinia());
  });

  it("路由进入项目页时自动建档,重复进入不重复建", async () => {
    await setRoute("/projects/3");
    const store = useTabsStore();
    expect(store.openProjectIds).toEqual([3]);

    await setRoute("/projects/5/files");
    expect(store.openProjectIds).toEqual([3, 5]);

    await setRoute("/projects/3");
    expect(store.openProjectIds).toEqual([3, 5]);
  });

  it("关闭后台 tab 只移除不导航,关闭激活 tab 切到相邻项目", async () => {
    await setRoute("/projects/1");
    const store = useTabsStore();
    store.openProject(2);

    expect(store.closeTab(2)).toBeNull();
    expect(store.openProjectIds).toEqual([1]);
    expect(pushMock).not.toHaveBeenCalled();

    // 激活的是 1,关闭后优先切到右侧相邻(此处 1 是最后一个,取左侧 3)
    store.openProject(3);
    expect(store.closeTab(1)).toEqual({ kind: "project", projectId: 3 });
    expect(pushMock).toHaveBeenCalledWith("/projects/3");
    expect(store.openProjectIds).toEqual([3]);
  });

  it("关闭最后一个激活 tab 时回首页", async () => {
    await setRoute("/projects/9");
    const store = useTabsStore();

    expect(store.closeTab(9)).toEqual({ kind: "home" });
    expect(pushMock).toHaveBeenCalledWith("/");
    expect(store.openProjectIds).toEqual([]);
  });

  it("reorderTabs 整体回写顺序并持久化到 localStorage", async () => {
    await setRoute("/");
    const setItem = vi.fn();
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: vi.fn(() => null),
      setItem,
    };
    const store = useTabsStore();
    store.openProject(1);
    store.openProject(2);
    store.openProject(3);
    expect(store.openProjectIds).toEqual([1, 2, 3]);

    store.reorderTabs([3, 1, 2]);
    expect(store.openProjectIds).toEqual([3, 1, 2]);
    expect(setItem).toHaveBeenLastCalledWith("repomeow.tabs.v1", JSON.stringify([3, 1, 2]));
  });

  it("打开的项目 tab 持久化到 localStorage 并在初始化时恢复", async () => {
    // mock 路由跨测试共享,先复位到首页,避免创建 store 时立即建档混入脏数据
    await setRoute("/");
    const setItem = vi.fn();
    (globalThis as { localStorage?: unknown }).localStorage = {
      getItem: vi.fn(() => JSON.stringify([4, 2])),
      setItem,
    };
    const store = useTabsStore();
    expect(store.openProjectIds).toEqual([4, 2]);

    store.openProject(8);
    expect(setItem).toHaveBeenCalledWith("repomeow.tabs.v1", JSON.stringify([4, 2, 8]));
  });
});
