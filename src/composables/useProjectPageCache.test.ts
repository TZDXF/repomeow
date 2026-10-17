import { describe, expect, it } from "vitest";
import { effectScope, nextTick, reactive, ref } from "vue";
import type { RouteLocationNormalizedLoaded } from "vue-router";
import { useProjectPageCache } from "@/composables/useProjectPageCache";

function route(path: string, query: Record<string, string> = {}) {
  return {
    path,
    fullPath: path + (Object.keys(query).length ? `?${new URLSearchParams(query)}` : ""),
    params: { id: path.split("/")[2] },
    query,
    matched: [],
    meta: {},
    hash: "",
    name: undefined,
  } as unknown as RouteLocationNormalizedLoaded;
}
function setup() {
  const scope = effectScope();
  const current = reactive(route("/projects/1"));
  const ids = ref([1, 2]);
  const pages = scope.run(() =>
    useProjectPageCache(
      () => current,
      () => ids.value,
    ),
  )!;
  async function navigate(path: string, query?: Record<string, string>) {
    Object.assign(current, route(path, query));
    await nextTick();
  }
  return { pages, ids, navigate, stop: () => scope.stop() };
}

describe("项目页面缓存", () => {
  it("仅缓存访问过的页面,切换项目不篡改原页面的路由和项目参数", async () => {
    const view = setup();
    expect(view.pages.value.map((page) => page.projectId)).toEqual([1]);
    const original = view.pages.value[0]!.route;
    await view.navigate("/projects/2");
    expect(view.pages.value).toHaveLength(2);
    expect(original.path).toBe("/projects/1");
    expect(original.params.id).toBe("1");
    await view.navigate("/projects/1");
    expect(view.pages.value).toHaveLength(2);
    view.stop();
  });

  it("子页分别缓存,同一子页 query 更新只替换路由快照", async () => {
    const view = setup();
    await view.navigate("/projects/1/files", { path: "README.md" });
    await view.navigate("/projects/1/files", { path: "package.json" });
    expect(view.pages.value).toHaveLength(2);
    expect(view.pages.value[1]!.route.query.path).toBe("package.json");
    view.stop();
  });

  it("切到首页、设置、报告时保留项目页面,但不把这些页面加入项目缓存", async () => {
    const view = setup();
    for (const path of ["/", "/settings", "/report-history"]) await view.navigate(path);
    expect(view.pages.value.map((page) => page.route.path)).toEqual(["/projects/1"]);
    view.stop();
  });

  it("关闭 tab 清理该项目全部子页,关闭激活 tab 时不因旧路由重新缓存", async () => {
    const view = setup();
    await view.navigate("/projects/1/files");
    await view.navigate("/projects/2");
    view.ids.value = [1];
    await nextTick();
    expect(view.pages.value.map((page) => page.route.path)).toEqual([
      "/projects/1",
      "/projects/1/files",
    ]);
    view.ids.value = [];
    await nextTick();
    expect(view.pages.value).toEqual([]);
    view.stop();
  });

  it("重新打开已关闭项目时创建新缓存项,不复用旧草稿实例", async () => {
    const view = setup();
    const original = view.pages.value[0];
    view.ids.value = [2];
    await view.navigate("/projects/2");
    view.ids.value = [2, 1];
    await view.navigate("/projects/1");
    expect(view.pages.value.find((page) => page.projectId === 1)).not.toBe(original);
    view.stop();
  });
});
