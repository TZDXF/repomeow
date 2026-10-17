import { describe, expect, it, vi } from "vitest";
import { effectScope, nextTick, ref } from "vue";
import { watchProjectPageDeactivation } from "@/composables/useProjectPageState";

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
