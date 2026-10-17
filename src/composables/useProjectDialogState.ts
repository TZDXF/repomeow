import { computed, ref, type Ref } from "vue";
import type { DialogRootProps } from "reka-ui";

/** 保留业务层开关,只在实际可见时启用 Reka 的焦点/滚动/外部交互锁。 */
export function useProjectDialogState(
  props: Readonly<DialogRootProps>,
  onUpdate: (open: boolean) => void,
  active?: Readonly<Ref<boolean>>,
) {
  const uncontrolledOpen = ref(props.defaultOpen ?? false);
  const requestedOpen = computed(() => props.open ?? uncontrolledOpen.value);
  const visible = computed(() => requestedOpen.value && (active?.value ?? true));
  // 切走时保留弹窗内部编辑器等组件;主动关闭时恢复原来的销毁行为。
  const unmountOnHide = computed(() =>
    active && requestedOpen.value ? false : props.unmountOnHide,
  );

  function updateOpen(value: boolean) {
    if (active && !active.value) return;
    uncontrolledOpen.value = value;
    onUpdate(value);
  }
  return { visible, unmountOnHide, updateOpen };
}
