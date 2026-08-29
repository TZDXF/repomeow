import { computed, nextTick, reactive } from "vue";
import { describe, expect, it } from "vitest";
import type { WikiPageStatus } from "@/lib/wiki-generator";
import type { WikiGenPageItem, WikiGenerationState } from "@/stores/wiki";
import type { WikiOutlinePage } from "@/types";
import { pickAutoPreviewPage, useWikiAutoPreview } from "./useWikiAutoPreview";

function pageItem(id: string, status: WikiPageStatus): WikiGenPageItem {
  const page: WikiOutlinePage = {
    id,
    file: `${id}.md`,
    title: id,
    description: "",
    section: null,
    importance: "high",
    relevantFiles: [],
    relatedPages: [],
  };
  return { page, status };
}

function makeState(pages: WikiGenPageItem[], streamContents: Record<string, string>) {
  return reactive({
    projectName: "demo",
    phase: "generating",
    pages,
    error: "",
    streamContents,
    context: null,
    toolCalls: 0,
    retries: {},
    startedAt: 0,
  }) as WikiGenerationState;
}

describe("pickAutoPreviewPage", () => {
  it("无可跟随页时取第一个已在输出的生成中页", () => {
    const pages = [pageItem("a", "running"), pageItem("b", "running")];
    expect(pickAutoPreviewPage(null, pages, { b: "content" })).toBe("b");
  });

  it("所有页都还没输出时取第一个生成中页", () => {
    const pages = [pageItem("a", "running"), pageItem("b", "running")];
    expect(pickAutoPreviewPage(null, pages, {})).toBe("a");
  });

  it("没有生成中的页时返回 null", () => {
    const pages = [pageItem("a", "done"), pageItem("b", "failed")];
    expect(pickAutoPreviewPage(null, pages, {})).toBeNull();
  });

  it("跟随页已有输出时粘住,即使其他页也在输出", () => {
    const pages = [pageItem("a", "running"), pageItem("b", "running")];
    expect(pickAutoPreviewPage("a", pages, { a: "x", b: "y" })).toBe("a");
  });

  it("跟随页字数为 0 而其他页已在输出时切到那个页", () => {
    const pages = [pageItem("a", "running"), pageItem("b", "running")];
    expect(pickAutoPreviewPage("a", pages, { b: "y" })).toBe("b");
  });

  it("跟随页字数为 0 且无其他页输出时保持等待", () => {
    const pages = [pageItem("a", "running"), pageItem("b", "running")];
    expect(pickAutoPreviewPage("a", pages, {})).toBe("a");
  });

  it("跟随页完成后切到第一个已在输出的生成中页", () => {
    const pages = [pageItem("a", "done"), pageItem("b", "running"), pageItem("c", "running")];
    expect(pickAutoPreviewPage("a", pages, { b: "y", c: "z" })).toBe("b");
  });

  it("跟随页完成后无页输出时取第一个生成中页", () => {
    const pages = [pageItem("a", "done"), pageItem("b", "running")];
    expect(pickAutoPreviewPage("a", pages, {})).toBe("b");
  });
});

describe("useWikiAutoPreview", () => {
  it("跨状态更新粘住输出中的页,完成后才前进到下一页", async () => {
    const state = makeState([pageItem("a", "running"), pageItem("b", "running")], { b: "partial" });
    const followedId = useWikiAutoPreview(computed(() => state));
    await nextTick();
    // a 还没输出而 b 在输出,初始跟随 b
    expect(followedId.value).toBe("b");
    // a 开始输出也不回切,b 未完成前保持粘住
    state.streamContents.a = "x";
    await nextTick();
    expect(followedId.value).toBe("b");
    // b 完成后才前进到 a
    const b = state.pages.find((item) => item.page.id === "b");
    if (b) {
      b.status = "done";
    }
    delete state.streamContents.b;
    await nextTick();
    expect(followedId.value).toBe("a");
  });

  it("跟随页迟迟无输出时让位给已在输出的页", async () => {
    const state = makeState([pageItem("a", "running"), pageItem("b", "running")], {});
    const followedId = useWikiAutoPreview(computed(() => state));
    await nextTick();
    expect(followedId.value).toBe("a");
    state.streamContents.b = "y";
    await nextTick();
    expect(followedId.value).toBe("b");
  });
});
