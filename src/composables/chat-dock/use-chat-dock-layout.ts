import { computed, nextTick, ref, useTemplateRef, watch, type WatchSource } from "vue";
import { useEventListener } from "@vueuse/core";

// --- 放大/还原与边缘拖拽:面板尺寸由响应式宽高驱动;默认 500x640,放大铺满高度 ---
// 高度让出自绘标题栏(TitleBar.vue h-9 = 2.25rem,z-60 盖在浮层 z-50 之上,
// 约定同 lib/popper.ts):底部边距 1rem + 标题栏下间隙 1rem + 标题栏 2.25rem
const PANEL_DEFAULT_WIDTH = 500;
const PANEL_DEFAULT_HEIGHT = 640;
const PANEL_EXPANDED_WIDTH = 720;
const PANEL_FALLBACK_MIN_WIDTH = 420;
const PANEL_MIN_HEIGHT = 320;
const PANEL_MARGIN = 68; // 2rem + 2.25rem
// 底栏固定占位:发送/停止槽 32 + 上下文圆圈约 28 + footer 横向 padding 16
// + 间隙 8 + 输入区 p-3 24 + 边框 2 ≈ 110;实测工具组 scrollWidth 后加上它
const PANEL_TOOLS_OVERHEAD = 110;

/**
 * ChatDock 面板布局:开合(ESC 收起)、放大/还原、边缘拖拽改尺寸、
 * 底栏最小宽度实测(保证权限/模型/思考强度 + 上下文圆圈 + 发送钮永不换行)。
 * measureTriggers 变化(模型/权限标签、语言)且面板打开时重测最小宽度。
 */
export function useChatDockLayout(measureTriggers: WatchSource<unknown>[]) {
  // --- 开合:收起为圆形入口,展开为右下角固定面板;ESC 或关闭钮收起 ---
  const open = ref(false);

  useEventListener(window, "keydown", (event: KeyboardEvent) => {
    if (open.value && event.key === "Escape") {
      event.stopPropagation();
      open.value = false;
    }
  });

  function toggleOpen() {
    open.value = !open.value;
  }

  const expanded = ref(false);
  const panelWidth = ref(PANEL_DEFAULT_WIDTH);
  const panelHeight = ref(PANEL_DEFAULT_HEIGHT);

  const maxPanelWidth = () => window.innerWidth - 32;
  const maxPanelHeight = () => window.innerHeight - PANEL_MARGIN;

  function toggleExpanded() {
    expanded.value = !expanded.value;
    if (expanded.value) {
      panelWidth.value = Math.min(PANEL_EXPANDED_WIDTH, maxPanelWidth());
      panelHeight.value = maxPanelHeight();
    } else {
      panelWidth.value = PANEL_DEFAULT_WIDTH;
      panelHeight.value = PANEL_DEFAULT_HEIGHT;
    }
  }

  // 最小宽度实测:临时把工具组脱离 flex 布局按内容量出自然单行宽度,保证底栏
  // (权限/模型/思考强度 + 上下文圆圈 + 发送钮)永不换行;标签随模型/语言变化,
  // 打开面板或相关值变化时重测
  const toolsRef = useTemplateRef<{ $el: HTMLElement } | null>("tools");
  const minPanelWidth = ref(PANEL_FALLBACK_MIN_WIDTH);

  async function measureMinPanelWidth() {
    await nextTick();
    const el = toolsRef.value?.$el;
    if (!el) return;
    // 不能靠叠 flex-nowrap 读 scrollWidth:Tailwind v4 里 .flex-wrap 层叠序晚于
    // .flex-nowrap(叠加无效),且 flex-1(basis:0%)下 scrollWidth 恒等于
    // clientWidth——量到的是当前分得的空间而非内容宽度,每次重测都会把
    // 「当前宽度 + 常量开销」写成新 min-width,切换下拉即不断变宽
    el.style.flex = "none";
    el.style.width = "max-content";
    const toolsWidth = el.getBoundingClientRect().width;
    el.style.flex = "";
    el.style.width = "";
    minPanelWidth.value = Math.max(
      PANEL_FALLBACK_MIN_WIDTH,
      Math.ceil(toolsWidth) + PANEL_TOOLS_OVERHEAD,
    );
  }

  watch([open, ...measureTriggers], () => {
    if (open.value) void measureMinPanelWidth();
  });

  const panelStyle = computed(() => ({
    width: `${panelWidth.value}px`,
    height: `${panelHeight.value}px`,
    minWidth: `${minPanelWidth.value}px`,
    maxWidth: "calc(100vw - 2rem)",
    maxHeight: "calc(100vh - 2rem - 2.25rem)",
  }));

  // 上边/左边/左上角拖拽:面板锚定右下角,拖左边加宽、拖上边加高;
  // 拖拽中禁用尺寸过渡,避免 0.25s 过渡滞后于指针
  const resizing = ref(false);

  function startResize(axis: "x" | "y" | "both", event: PointerEvent) {
    if (event.button !== 0) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    const startX = event.clientX;
    const startY = event.clientY;
    const startWidth = panelWidth.value;
    const startHeight = panelHeight.value;
    resizing.value = true;
    handle.setPointerCapture(event.pointerId);
    const onMove = (e: PointerEvent) => {
      expanded.value = false;
      if (axis !== "y") {
        panelWidth.value = Math.min(
          Math.max(startWidth + (startX - e.clientX), minPanelWidth.value),
          maxPanelWidth(),
        );
      }
      if (axis !== "x") {
        panelHeight.value = Math.min(
          Math.max(startHeight + (startY - e.clientY), PANEL_MIN_HEIGHT),
          maxPanelHeight(),
        );
      }
    };
    const onUp = () => {
      resizing.value = false;
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  return {
    open,
    toggleOpen,
    expanded,
    toggleExpanded,
    panelStyle,
    resizing,
    startResize,
    toolsRef,
  };
}
