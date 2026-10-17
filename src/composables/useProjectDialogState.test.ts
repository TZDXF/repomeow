import { describe, expect, it, vi } from "vitest";
import { reactive, ref } from "vue";
import type { DialogRootProps } from "reka-ui";
import { useProjectDialogState } from "@/composables/useProjectDialogState";

function dialog(initialOpen = true) {
  const props = reactive<DialogRootProps>({ open: initialOpen });
  const active = ref(true);
  const update = vi.fn((open: boolean) => {
    props.open = open;
  });
  return { props, active, update, ...useProjectDialogState(props, update, active) };
}

describe("项目弹窗状态", () => {
  it("切走隐藏且保留逻辑开关/内容,切回恢复,不触发业务层的关闭与重置 watcher", () => {
    const view = dialog();
    expect(view.visible.value).toBe(true);
    expect(view.unmountOnHide.value).toBe(false);
    view.active.value = false;
    expect(view.visible.value).toBe(false);
    expect(view.props.open).toBe(true);
    expect(view.unmountOnHide.value).toBe(false);
    expect(view.update).not.toHaveBeenCalled();
    view.active.value = true;
    expect(view.visible.value).toBe(true);
    expect(view.update).not.toHaveBeenCalled();
  });

  it("后台页面收到 dismiss 不丢失待恢复的弹窗", () => {
    const view = dialog();
    view.active.value = false;
    view.updateOpen(false);
    expect(view.props.open).toBe(true);
    expect(view.update).not.toHaveBeenCalled();
  });

  it("主动关闭后不再恢复,并恢复默认的卸载行为", () => {
    const view = dialog();
    view.updateOpen(false);
    expect(view.update).toHaveBeenCalledWith(false);
    expect(view.unmountOnHide.value).toBeUndefined();
    view.active.value = false;
    view.active.value = true;
    expect(view.visible.value).toBe(false);
  });

  it("两个项目可分别保留开关,互不覆盖", () => {
    const a = dialog();
    const b = dialog(false);
    a.active.value = false;
    b.updateOpen(true);
    expect(a.visible.value).toBe(false);
    expect(b.visible.value).toBe(true);
    b.active.value = false;
    a.active.value = true;
    expect(a.visible.value).toBe(true);
    expect(b.visible.value).toBe(false);
  });

  it("后台任务主动更新业务开关时,返回后遵循最新结果", () => {
    const view = dialog();
    view.active.value = false;
    view.props.open = false;
    view.active.value = true;
    expect(view.visible.value).toBe(false);
  });

  it("无项目作用域时保留普通弹窗和 unmountOnHide 配置", () => {
    const props = reactive<DialogRootProps>({ open: true, unmountOnHide: true });
    const update = vi.fn();
    const view = useProjectDialogState(props, update);
    expect(view.visible.value).toBe(true);
    expect(view.unmountOnHide.value).toBe(true);
    view.updateOpen(false);
    expect(update).toHaveBeenCalledWith(false);
  });

  it("兼容 defaultOpen 和未受控的触发器开关", () => {
    const active = ref(true);
    const view = useProjectDialogState({ defaultOpen: true }, vi.fn(), active);
    expect(view.visible.value).toBe(true);
    active.value = false;
    expect(view.visible.value).toBe(false);
    active.value = true;
    expect(view.visible.value).toBe(true);
    view.updateOpen(false);
    expect(view.visible.value).toBe(false);
    view.updateOpen(true);
    expect(view.visible.value).toBe(true);
  });
});
