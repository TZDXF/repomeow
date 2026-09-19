import { ref, watch, type ComputedRef, type Ref } from "vue";
import type { WikiGenPageItem, WikiGenerationState } from "@/stores/wiki";

/** 首次优先预览已有输出的页面；选定后保持不变，不随并发进度切页。 */
export function pickAutoPreviewPage(
  followedId: string | null,
  pages: WikiGenPageItem[],
  streamContents: Record<string, string>,
): string | null {
  const followed = pages.find((item) => item.page.id === followedId);
  if (followed) return followed.page.id;
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
