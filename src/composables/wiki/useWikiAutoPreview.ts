import { ref, watch, type ComputedRef, type Ref } from "vue";
import type { WikiGenPageItem, WikiGenerationState } from "@/stores/wiki";

/**
 * 自动预览页挑选规则(纯函数,便于单测):
 * - 跟随页仍在生成中且已有输出(字数 > 0)时粘住不动,直到它进入终态;
 * - 跟随页字数为 0(还没等到首块)而其他生成中页已在输出时,切到那个页,避免干等;
 * - 无可跟随页(未开始/已完成/不在列表)时,优先取第一个已在输出的生成中页,
 *   再退回第一个生成中的页。
 */
export function pickAutoPreviewPage(
  followedId: string | null,
  pages: WikiGenPageItem[],
  streamContents: Record<string, string>,
): string | null {
  const followed = pages.find((item) => item.page.id === followedId);
  if (followed?.status === "running") {
    if (streamContents[followed.page.id]) {
      return followed.page.id;
    }
    const outputting = pages.find(
      (item) => item.status === "running" && streamContents[item.page.id],
    );
    if (outputting) {
      return outputting.page.id;
    }
    return followed.page.id;
  }
  return (
    pages.find((item) => item.status === "running" && streamContents[item.page.id])?.page.id ??
    pages.find((item) => item.status === "running")?.page.id ??
    null
  );
}

/**
 * 自动预览的粘性跟随:跨响应式更新记住当前跟随的页,生成状态变化时用
 * pickAutoPreviewPage 重新评估,只有规则判定要换页时才更新跟随目标。
 */
export function useWikiAutoPreview(
  generation: ComputedRef<WikiGenerationState | undefined>,
): Ref<string | null> {
  const followedId = ref<string | null>(null);
  watch(
    () => {
      const state = generation.value;
      return state
        ? pickAutoPreviewPage(followedId.value, state.pages, state.streamContents)
        : null;
    },
    (id) => {
      followedId.value = id;
    },
    { immediate: true },
  );
  return followedId;
}
