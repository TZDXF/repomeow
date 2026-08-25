import { nextTick, watch, type Ref } from "vue";

interface WikiPreviewScrollOptions {
  generating: Readonly<Ref<boolean>>;
  activePreviewId: Readonly<Ref<string | null | undefined>>;
  previewContent: Readonly<Ref<string>>;
  activityCount: Readonly<Ref<number>>;
  previewHost: Ref<HTMLElement | null>;
  activityLogHost: Ref<HTMLElement | null>;
}

/**
 * 管理 Wiki 流式预览的滚动语义：默认跟随到底部，用户上翻时暂停，回到底部恢复；
 * 切换预览页或进入/结束生成时回到顶部并重新启用跟随。
 */
export function useWikiPreviewScroll(options: WikiPreviewScrollOptions) {
  let pinnedToBottom = true;
  let suppressScrollEvents = false;

  /** reka-ui ScrollArea 的实际滚动元素是内部 viewport。 */
  function scrollViewport(): HTMLElement | null {
    return options.previewHost.value?.closest('[data-slot="scroll-area-viewport"]') ?? null;
  }

  function onPreviewScroll(event: Event) {
    if (suppressScrollEvents) {
      return;
    }
    const element = event.target as HTMLElement;
    pinnedToBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
  }

  /** 程序化滚动屏蔽自身触发的事件，避免覆盖用户的 pinned 状态。 */
  function setViewportScroll(viewport: HTMLElement, position: "top" | "bottom") {
    suppressScrollEvents = true;
    viewport.scrollTop = position === "top" ? 0 : viewport.scrollHeight;
    requestAnimationFrame(() => {
      suppressScrollEvents = false;
    });
  }

  watch(options.previewHost, (element, _previous, onCleanup) => {
    if (!element) {
      return;
    }
    const viewport = scrollViewport();
    if (!viewport) {
      return;
    }
    viewport.addEventListener("scroll", onPreviewScroll, { passive: true });
    onCleanup(() => viewport.removeEventListener("scroll", onPreviewScroll));
  });

  watch(options.previewContent, async () => {
    if (!options.generating.value || !pinnedToBottom) {
      return;
    }
    await nextTick();
    const viewport = scrollViewport();
    if (viewport) {
      setViewportScroll(viewport, "bottom");
    }
  });

  watch(options.activityCount, async () => {
    await nextTick();
    const element = options.activityLogHost.value;
    if (element) {
      element.scrollTop = element.scrollHeight;
    }
  });

  watch(options.activityLogHost, async (element) => {
    if (!element) {
      return;
    }
    await nextTick();
    element.scrollTop = element.scrollHeight;
  });

  watch(
    () =>
      [
        options.generating.value,
        options.generating.value ? options.activePreviewId.value : null,
      ] as const,
    async () => {
      pinnedToBottom = true;
      await nextTick();
      const viewport = scrollViewport();
      if (viewport) {
        setViewportScroll(viewport, "top");
      }
    },
  );

  async function scrollPreviewToTop() {
    await nextTick();
    const viewport = scrollViewport();
    if (viewport) {
      setViewportScroll(viewport, "top");
    }
  }

  return { scrollPreviewToTop };
}
