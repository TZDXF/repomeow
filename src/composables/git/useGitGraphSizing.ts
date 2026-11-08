import { computed, type Ref } from "vue";
import { useElementSize, useLocalStorage } from "@vueuse/core";

const LANE_WIDTH = 16,
  GRAPH_PADDING = 4,
  GRAPH_DEFAULT_LANES = 5,
  GRAPH_COLUMN_MIN_WIDTH = LANE_WIDTH + GRAPH_PADDING * 2,
  COLUMN_MIN_WIDTH = { desc: 160, author: 64, commit: 80, date: 96 } as const,
  DETAIL_MIN_WIDTH = 320,
  TABLE_MIN_WIDTH = 480;

export type GitGraphColumnKey = "graph" | "desc" | "author" | "commit" | "date";

export function useGitGraphColumnSizing(containerWidth: Ref<number>, graphWidth: Ref<number>) {
  const colWidths = useLocalStorage(
      "repomeow:graph-col-widths",
      { graph: 0, descDelta: 0, author: 120, commit: 96, date: 150 },
      { mergeDefaults: true },
    ),
    graphColWidth = computed(() =>
      colWidths.value.graph > 0
        ? Math.max(colWidths.value.graph, GRAPH_COLUMN_MIN_WIDTH)
        : Math.min(graphWidth.value, GRAPH_DEFAULT_LANES * LANE_WIDTH + GRAPH_PADDING * 2),
    ),
    graphClipPath = computed(() => {
      const overflow = graphWidth.value - graphColWidth.value;
      return overflow > 0 ? `inset(-9999px ${overflow}px -9999px -9999px)` : "none";
    }),
    descRestWidth = computed(
      () =>
        containerWidth.value -
        graphColWidth.value -
        colWidths.value.author -
        colWidths.value.commit -
        colWidths.value.date,
    ),
    descColWidth = computed(() =>
      Math.max(descRestWidth.value + colWidths.value.descDelta, COLUMN_MIN_WIDTH.desc),
    ),
    totalWidth = computed(
      () =>
        graphColWidth.value +
        descColWidth.value +
        colWidths.value.author +
        colWidths.value.commit +
        colWidths.value.date,
    );

  function colWidth(key: GitGraphColumnKey) {
    if (key === "graph") {
      return graphColWidth.value;
    }
    if (key === "desc") {
      return descColWidth.value;
    }
    return colWidths.value[key];
  }

  function startColResize(key: GitGraphColumnKey, event: PointerEvent) {
    event.preventDefault();
    const startX = event.clientX,
      startWidth = colWidth(key),
      minWidth = key === "graph" ? GRAPH_COLUMN_MIN_WIDTH : COLUMN_MIN_WIDTH[key],
      restAtStart = descRestWidth.value,
      onMove = (moveEvent: PointerEvent) => {
        const target = Math.max(minWidth, Math.round(startWidth + moveEvent.clientX - startX));
        if (key === "desc") {
          colWidths.value.descDelta = target - restAtStart;
        } else {
          colWidths.value[key] = target;
        }
      },
      onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
      };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  return {
    colWidths,
    graphColWidth,
    graphClipPath,
    descColWidth,
    totalWidth,
    startColResize,
  };
}

export function useGitGraphDetailSizing(
  hasSidebar: Ref<boolean>,
  sidebarOpen: Ref<boolean>,
  mainRowEl: Ref<HTMLElement | null>,
) {
  const detailWidth = useLocalStorage("repomeow:graph-detail-width", 480),
    detailOpen = useLocalStorage("repomeow:graph-detail-open", true),
    { width: mainRowWidth } = useElementSize(mainRowEl),
    detailMaxWidth = computed(() => {
      if (!mainRowWidth.value) {
        return DETAIL_MIN_WIDTH;
      }
      let sidebarWidth = 0;
      if (hasSidebar.value) {
        sidebarWidth = sidebarOpen.value ? 224 : 32;
      }
      return Math.max(
        DETAIL_MIN_WIDTH,
        Math.floor(mainRowWidth.value) - sidebarWidth - TABLE_MIN_WIDTH,
      );
    }),
    effectiveDetailWidth = computed(() => Math.min(detailWidth.value, detailMaxWidth.value));

  function startDetailResize(event: PointerEvent) {
    event.preventDefault();
    const startX = event.clientX,
      startWidth = effectiveDetailWidth.value,
      maxWidth = detailMaxWidth.value,
      onMove = (moveEvent: PointerEvent) => {
        detailWidth.value = Math.min(
          maxWidth,
          Math.max(DETAIL_MIN_WIDTH, Math.round(startWidth - (moveEvent.clientX - startX))),
        );
      },
      onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
      };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }

  return { detailOpen, effectiveDetailWidth, startDetailResize };
}
