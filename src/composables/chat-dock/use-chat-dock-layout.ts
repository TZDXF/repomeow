import { computed, onUnmounted, ref, useTemplateRef, watch, type Ref } from "vue";
import { useEventListener } from "@vueuse/core";
import { useProjectPageActive } from "@/composables/useProjectPageState";
import { clampChatSidebarWidth } from "@/lib/chat-layout";

const PANEL_DEFAULT_WIDTH = 500;
const PANEL_EXPANDED_WIDTH = 720;

/** 嵌入项目工作区的右侧栏:开合、加宽/还原、左边缘拖拽;高度随父容器。 */
export function useChatDockLayout(open: Ref<boolean>) {
  const active = useProjectPageActive();
  const panel = useTemplateRef<HTMLElement>("chatPanel");
  const expanded = ref(false);
  const panelWidth = ref(PANEL_DEFAULT_WIDTH);
  const resizing = ref(false);
  let stopResize: (() => void) | undefined;

  useEventListener(window, "keydown", (event: KeyboardEvent) => {
    if (active.value && open.value && event.key === "Escape" && !event.defaultPrevented) {
      event.stopPropagation();
      open.value = false;
    }
  });

  function toggleExpanded() {
    expanded.value = !expanded.value;
    panelWidth.value = expanded.value ? PANEL_EXPANDED_WIDTH : PANEL_DEFAULT_WIDTH;
  }

  // 百分比相对主工作区而非浏览器视口,窗口缩放及项目页缓存恢复时自动适配。
  // 与 chatSidebarBounds 保持一致,不能让 min-width 覆盖窄窗口下的 max-width。
  const panelStyle = computed(() => ({
    width: `${panelWidth.value}px`,
    minWidth: "min(420px, calc(100% - min(360px, 40%)))",
    maxWidth: "calc(100% - min(360px, 40%))",
  }));

  function startResize(event: PointerEvent) {
    if (event.button !== 0 || !panel.value) return;
    event.preventDefault();
    stopResize?.();
    const handle = event.currentTarget as HTMLElement;
    const container = panel.value.parentElement;
    if (!container) return;
    const startX = event.clientX;
    // 从实际渲染宽度开始,避免窗口收窄后第一次拖动突然跳变。
    const startWidth = panel.value.getBoundingClientRect().width;
    resizing.value = true;
    handle.setPointerCapture(event.pointerId);
    const onMove = (e: PointerEvent) => {
      expanded.value = false;
      panelWidth.value = clampChatSidebarWidth(
        startWidth + startX - e.clientX,
        container.clientWidth,
      );
    };
    const onUp = () => {
      resizing.value = false;
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      handle.removeEventListener("lostpointercapture", onUp);
      if (handle.hasPointerCapture(event.pointerId)) handle.releasePointerCapture(event.pointerId);
      stopResize = undefined;
    };
    stopResize = onUp;
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
    handle.addEventListener("lostpointercapture", onUp);
  }

  watch([open, active], () => {
    if (!open.value || !active.value) stopResize?.();
  });
  onUnmounted(() => stopResize?.());

  return { expanded, toggleExpanded, panelStyle, resizing, startResize };
}
