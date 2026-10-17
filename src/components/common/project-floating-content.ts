import { defineComponent, h, inject, type Component } from "vue";
import {
  injectContextMenuRootContext,
  injectDropdownMenuRootContext,
  injectPopoverRootContext,
  injectSelectRootContext,
} from "reka-ui";
import ContextContent from "@/components/ui/context-menu/ContextMenuContent.vue";
import DropdownContent from "@/components/ui/dropdown-menu/DropdownMenuContent.vue";
import PopoverContentBase from "@/components/ui/popover/PopoverContent.vue";
import SelectContentBase from "@/components/ui/select/SelectContent.vue";
import {
  PROJECT_PAGE_ACTIVE,
  watchProjectPageDeactivation,
} from "@/composables/useProjectPageState";

/** 临时菜单不需要恢复,切走即关闭,防止隐藏菜单仍锁住 body 的鼠标事件或焦点。 */
function scopedContent<T extends Component>(
  content: T,
  context: () => { onOpenChange: (open: boolean) => void },
): T {
  // 保留原组件的 props/事件类型,所有 attrs、事件和插槽仍交给原 UI 组件处理。
  return defineComponent({
    inheritAttrs: false,
    setup(_, { attrs, slots }) {
      const active = inject(PROJECT_PAGE_ACTIVE, undefined);
      if (active) {
        const { onOpenChange } = context();
        watchProjectPageDeactivation(active, () => onOpenChange(false));
      }
      // 隐藏祖先时退出动画不会结束,直接卸载临时浮层,确保全局锁立即释放。
      return () => (!active || active.value ? h(content, attrs, slots) : null);
    },
  }) as T;
}

export const ContextMenuContent = scopedContent(ContextContent, injectContextMenuRootContext);
export const DropdownMenuContent = scopedContent(DropdownContent, injectDropdownMenuRootContext);
export const PopoverContent = scopedContent(PopoverContentBase, injectPopoverRootContext);
export const SelectContent = scopedContent(SelectContentBase, injectSelectRootContext);
