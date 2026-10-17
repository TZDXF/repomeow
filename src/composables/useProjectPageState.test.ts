import { describe, expect, it, vi } from "vitest";
import { effectScope, nextTick, ref } from "vue";
import {
  watchProjectPageDeactivation,
  watchProjectPageReactivation,
} from "@/composables/useProjectPageState";

describe("项目页面的临时浮层", () => {
  it("切走关闭菜单释放全局交互锁,切回不自动重新打开临时菜单", async () => {
    const active = ref(true);
    const close = vi.fn();
    const stop = watchProjectPageDeactivation(active, close);
    expect(close).not.toHaveBeenCalled();
    active.value = false;
    await nextTick();
    expect(close).toHaveBeenCalledTimes(1);
    active.value = true;
    await nextTick();
    expect(close).toHaveBeenCalledTimes(1);
    stop();
  });

  it("后台页面异步挂载的浮层也立即关闭", () => {
    const close = vi.fn();
    const stop = watchProjectPageDeactivation(ref(false), close);
    expect(close).toHaveBeenCalledOnce();
    stop();
  });

  it("浮层卸载后不残留对后台 tab 的订阅", async () => {
    const active = ref(true);
    const close = vi.fn();
    const scope = effectScope();
    scope.run(() => watchProjectPageDeactivation(active, close));
    scope.stop();
    active.value = false;
    await nextTick();
    expect(close).not.toHaveBeenCalled();
  });
});

describe("缓存页面重新激活的数据刷新", () => {
  it("初次挂载不重复加载,切走不刷新,每次返回刷新一次", async () => {
    const active = ref(true);
    const refresh = vi.fn();
    const stop = watchProjectPageReactivation(active, refresh);
    await nextTick();
    expect(refresh).not.toHaveBeenCalled();
    active.value = false;
    await nextTick();
    expect(refresh).not.toHaveBeenCalled();
    active.value = true;
    await nextTick();
    expect(refresh).toHaveBeenCalledTimes(1);
    active.value = false;
    await nextTick();
    active.value = true;
    await nextTick();
    expect(refresh).toHaveBeenCalledTimes(2);
    stop();
  });

  it("后台挂载的页面在首次变为可见时刷新最新数据", async () => {
    const active = ref(false);
    const refresh = vi.fn(async () => {});
    const stop = watchProjectPageReactivation(active, refresh);
    expect(refresh).not.toHaveBeenCalled();
    active.value = true;
    await nextTick();
    expect(refresh).toHaveBeenCalledOnce();
    stop();
  });

  it("销毁缓存页面后不再响应激活,刷新不重置页面草稿", async () => {
    const active = ref(true);
    const draft = ref("未提交的草稿");
    const refresh = vi.fn();
    const scope = effectScope();
    scope.run(() => watchProjectPageReactivation(active, refresh));
    active.value = false;
    await nextTick();
    active.value = true;
    await nextTick();
    expect(draft.value).toBe("未提交的草稿");
    expect(refresh).toHaveBeenCalledOnce();
    scope.stop();
    active.value = false;
    await nextTick();
    active.value = true;
    await nextTick();
    expect(refresh).toHaveBeenCalledOnce();
  });
});
