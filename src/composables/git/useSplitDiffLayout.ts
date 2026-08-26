import { computed, nextTick, onBeforeUnmount, ref, watch, type ComputedRef, type Ref } from "vue";
import { useElementSize } from "@vueuse/core";
import type { DiffSideRow } from "@/lib/diff";
import {
  buildChangeBlocks,
  buildDividerShapes,
  buildInsertMarkers,
  buildPaneRowOffsets,
  buildPaneRows,
  locatePaneRowPosition,
  paneScrollTopAt,
  type DiffPaneSide,
} from "@/lib/diff-viewer";

type PaneScrollSource = DiffPaneSide | "leftGutter" | "rightGutter";

interface SplitDiffLayoutOptions {
  sideRows: ComputedRef<DiffSideRow[]>;
  layoutRows: ComputedRef<readonly unknown[]>;
  splitActive: ComputedRef<boolean>;
  landedDiff: Ref<unknown>;
  splitRatio: Ref<number>;
  currentRowPos: Ref<number>;
  elements: SplitDiffLayoutElements;
}

export interface SplitDiffLayoutElements {
  splitWrapEl: Ref<HTMLElement | null>;
  leftPaneEl: Ref<HTMLElement | null>;
  rightPaneEl: Ref<HTMLElement | null>;
  leftGutterEl: Ref<HTMLElement | null>;
  rightGutterEl: Ref<HTMLElement | null>;
  dividerEl: Ref<HTMLElement | null>;
}

const SPLIT_RATIO_MIN = 0.2;
const SPLIT_RATIO_MAX = 0.8;

/**
 * 并排 diff 的 DOM 布局控制器：维护左右内容/行号栏滚动同步、连接条图形、
 * 横向滚动条补偿与分隔条拖拽。行模型和坐标换算委托给纯函数，便于独立测试。
 */
export function useSplitDiffLayout(options: SplitDiffLayoutOptions) {
  const { sideRows, layoutRows, splitActive, landedDiff, splitRatio, currentRowPos, elements } =
    options;
  const { splitWrapEl, leftPaneEl, rightPaneEl, leftGutterEl, rightGutterEl, dividerEl } = elements;

  const leftRows = computed(() => buildPaneRows(sideRows.value, "left"));
  const rightRows = computed(() => buildPaneRows(sideRows.value, "right"));
  const paneRowOffsets = computed(() => buildPaneRowOffsets(sideRows.value));
  const changeBlocks = computed(() => buildChangeBlocks(sideRows.value));
  const leftMarkers = computed(() =>
    buildInsertMarkers(changeBlocks.value, paneRowOffsets.value, "left"),
  );
  const rightMarkers = computed(() =>
    buildInsertMarkers(changeBlocks.value, paneRowOffsets.value, "right"),
  );

  const leftScrollPx = ref(0);
  const rightScrollPx = ref(0);
  const paneSyncing = ref(false);
  let paneSyncFrame = 0;

  function rowHeightPx() {
    return parseFloat(getComputedStyle(document.documentElement).fontSize) * 1.25 || 20;
  }

  function locateRowPos(side: DiffPaneSide, scrollTop: number) {
    return locatePaneRowPosition(paneRowOffsets.value[side], scrollTop, rowHeightPx());
  }

  function scrollTopAt(side: DiffPaneSide, rowPosition: number) {
    return paneScrollTopAt(paneRowOffsets.value[side], rowPosition, rowHeightPx());
  }

  // 左窗格 direction:rtl 下 scrollLeft 是负值；统一换算为距行首的可视偏移。
  function visualScrollLeft(element: HTMLElement) {
    return element === leftPaneEl.value
      ? element.scrollWidth - element.clientWidth + element.scrollLeft
      : element.scrollLeft;
  }

  function applyVisualScrollLeft(element: HTMLElement, offset: number) {
    element.scrollLeft =
      element === leftPaneEl.value ? offset - (element.scrollWidth - element.clientWidth) : offset;
  }

  function syncPaneScroll(source: PaneScrollSource) {
    if (leftPaneEl.value) {
      leftScrollPx.value = leftPaneEl.value.scrollTop;
    }
    if (rightPaneEl.value) {
      rightScrollPx.value = rightPaneEl.value.scrollTop;
    }
    if (paneSyncing.value) {
      return;
    }

    const side: DiffPaneSide = source === "left" || source === "leftGutter" ? "left" : "right";
    const other: DiffPaneSide = side === "left" ? "right" : "left";
    const panes = { left: leftPaneEl, right: rightPaneEl };
    const gutters = { left: leftGutterEl, right: rightGutterEl };
    const fromGutter = source === "leftGutter" || source === "rightGutter";
    const from = (fromGutter ? gutters[side] : panes[side]).value;
    if (!from) {
      return;
    }

    const rowPosition = locateRowPos(side, from.scrollTop);
    currentRowPos.value = rowPosition;
    paneSyncing.value = true;

    const mate = (fromGutter ? panes[side] : gutters[side]).value;
    if (mate) {
      mate.scrollTop = from.scrollTop;
    }
    const mapped = scrollTopAt(other, rowPosition);
    if (panes[other].value) {
      panes[other].value.scrollTop = mapped;
    }
    if (gutters[other].value) {
      gutters[other].value.scrollTop = mapped;
    }
    if (!fromGutter && panes[other].value) {
      applyVisualScrollLeft(panes[other].value, visualScrollLeft(from));
    }
    paneSyncFrame = requestAnimationFrame(() => {
      paneSyncing.value = false;
    });
  }

  const { width: dividerWidth, height: dividerHeight } = useElementSize(dividerEl);
  const dividerShapes = computed(() =>
    buildDividerShapes({
      rows: sideRows.value,
      blocks: changeBlocks.value,
      offsets: paneRowOffsets.value,
      width: dividerWidth.value || 20,
      viewportHeight: dividerHeight.value,
      leftScrollTop: leftScrollPx.value,
      rightScrollTop: rightScrollPx.value,
      rowHeight: rowHeightPx(),
    }),
  );

  // 新内容落地或刚切换为并排视图时，两侧都复位到可视行首。
  watch(
    [splitActive, landedDiff],
    async ([active]) => {
      if (!active) {
        return;
      }
      await nextTick();
      const leftPane = leftPaneEl.value;
      if (leftPane) {
        leftPane.scrollLeft = -(leftPane.scrollWidth - leftPane.clientWidth);
      }
    },
    { immediate: true },
  );

  const hbarPad = ref({ leftGutter: 0, rightGutter: 0 });

  async function syncHbarPad() {
    await nextTick();
    hbarPad.value = {
      leftGutter: leftPaneEl.value
        ? leftPaneEl.value.offsetHeight - leftPaneEl.value.clientHeight
        : 0,
      rightGutter: rightPaneEl.value
        ? rightPaneEl.value.offsetHeight - rightPaneEl.value.clientHeight
        : 0,
    };
  }

  const { width: viewerWidth, height: viewerHeight } = useElementSize(splitWrapEl);
  watch(
    [layoutRows, splitActive, viewerWidth, viewerHeight, splitRatio],
    () => void syncHbarPad(),
    { flush: "post" },
  );

  let splitResizeCleanups: (() => void)[] = [];

  function startSplitResize(event: PointerEvent) {
    event.preventDefault();
    const wrap = splitWrapEl.value;
    const leftGutter = leftGutterEl.value;
    const rightGutter = rightGutterEl.value;
    const divider = dividerEl.value;
    if (!wrap || !leftGutter || !rightGutter || !divider) {
      return;
    }
    const paneArea =
      wrap.clientWidth - leftGutter.offsetWidth - rightGutter.offsetWidth - divider.offsetWidth;
    if (paneArea <= 0) {
      return;
    }
    const baseX =
      wrap.getBoundingClientRect().left + leftGutter.offsetWidth + divider.offsetWidth / 2;
    const onMove = (moveEvent: PointerEvent) => {
      splitRatio.value = Math.min(
        SPLIT_RATIO_MAX,
        Math.max(SPLIT_RATIO_MIN, (moveEvent.clientX - baseX) / paneArea),
      );
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      splitResizeCleanups = splitResizeCleanups.filter((cleanup) => cleanup !== onUp);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    splitResizeCleanups.push(onUp);
  }

  onBeforeUnmount(() => {
    for (const cleanup of splitResizeCleanups) {
      cleanup();
    }
    splitResizeCleanups = [];
    if (paneSyncFrame) {
      cancelAnimationFrame(paneSyncFrame);
    }
  });

  return {
    leftRows,
    rightRows,
    leftMarkers,
    rightMarkers,
    dividerShapes,
    hbarPad,
    rowHeightPx,
    scrollTopAt,
    syncPaneScroll,
    startSplitResize,
  };
}
