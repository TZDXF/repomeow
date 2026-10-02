import { describe, expect, it } from "vitest";
import type { ChatMessage } from "@/lib/chat";
import type { ChatSessionState } from "@/stores/chat";
import { buildTimeline, retryRemainingSeconds, type TurnView } from "./use-chat-timeline";

function msg(partial: Partial<ChatMessage> & Pick<ChatMessage, "id" | "role">): ChatMessage {
  return { content: "", toolRunIds: [], ...partial };
}

function session(partial: Partial<ChatSessionState>): ChatSessionState {
  return {
    messages: [],
    phase: "idle",
    streamingText: "",
    streamingThinking: "",
    pendingToolRunIds: [],
    toolRuns: {},
    error: null,
    busy: false,
    lastUsage: null,
    contextTokens: null,
    contextBreakdown: null,
    cacheHitInputTokens: 0,
    cacheHitCachedTokens: 0,
    retry: null,
    compacting: false,
    ...partial,
  };
}

describe("buildTimeline", () => {
  it("用户消息独立成视图,连续 assistant 消息归并为一个回合", () => {
    const views = buildTimeline(
      session({
        messages: [
          msg({ id: "u1", role: "user", content: "问题" }),
          msg({ id: "a1", role: "assistant", content: "第一段" }),
          msg({ id: "a2", role: "assistant", content: "第二段" }),
        ],
      }),
    );
    expect(views.map((v) => v.kind)).toEqual(["user", "turn"]);
    const turn = views[1] as TurnView;
    expect(turn.key).toBe("a1");
    expect(turn.contents).toEqual(["第一段", "第二段"]);
    expect(turn.live).toBe(false);
  });

  it("仅最后一条用户提问可编辑;忙时一律不可编辑", () => {
    const views = buildTimeline(
      session({
        messages: [
          msg({ id: "u1", role: "user", content: "旧" }),
          msg({ id: "a1", role: "assistant", content: "答" }),
          msg({ id: "u2", role: "user", content: "新" }),
        ],
      }),
    );
    expect(views[0]).toMatchObject({ kind: "user", editable: false });
    expect(views[2]).toMatchObject({ kind: "user", editable: true });

    const busyViews = buildTimeline(
      session({ busy: true, messages: [msg({ id: "u1", role: "user", content: "x" })] }),
    );
    expect(busyViews[0]).toMatchObject({ kind: "user", editable: false });
  });

  it("compaction 消息产出分隔标记并打断回合归并", () => {
    const views = buildTimeline(
      session({
        messages: [
          msg({ id: "a1", role: "assistant", content: "前" }),
          msg({
            id: "c1",
            role: "assistant",
            compaction: { tokensBefore: 9000, tokensAfter: 3000 },
          }),
          msg({ id: "a2", role: "assistant", content: "后" }),
        ],
      }),
    );
    expect(views.map((v) => v.kind)).toEqual(["turn", "compaction", "turn"]);
    expect(views[1]).toMatchObject({ tokensBefore: 9000, tokensAfter: 3000 });
  });

  it("思考与工具调用进过程组;纯空白正文不进渲染列表", () => {
    const views = buildTimeline(
      session({
        messages: [
          msg({
            id: "a1",
            role: "assistant",
            content: "\n\n",
            thinking: "想一下",
            toolRunIds: ["r1"],
          }),
          msg({ id: "a2", role: "assistant", content: "正文" }),
        ],
        toolRuns: {
          r1: { id: "r1", name: "read", args: {}, ok: true, summary: "ok", permission: null },
        },
      }),
    );
    const turn = views[0] as TurnView;
    expect(turn.contents).toEqual(["正文"]);
    expect(turn.groups).toHaveLength(1);
    expect(turn.groups[0].thinking).toBe("想一下");
    expect(turn.groups[0].runs.map((r) => r.id)).toEqual(["r1"]);
  });

  it("忙时流式状态并入末尾回合:正文已开始则过程块收起", () => {
    const views = buildTimeline(
      session({
        busy: true,
        streamingText: "流式中",
        pendingToolRunIds: ["r1"],
        toolRuns: {
          r1: { id: "r1", name: "bash", args: {}, ok: null, summary: "", permission: null },
        },
        messages: [
          msg({ id: "u1", role: "user", content: "问" }),
          msg({ id: "a1", role: "assistant", content: "已固化" }),
        ],
      }),
    );
    expect(views).toHaveLength(2);
    const turn = views[1] as TurnView;
    expect(turn.live).toBe(true);
    expect(turn.streamingText).toBe("流式中");
    expect(turn.contents).toEqual(["已固化"]);
    // 有待固化工具调用 → 过程仍在产出,折叠块保持展开
    expect(turn.active).toBe(true);
    expect(turn.groups[turn.groups.length - 1].runs.map((r) => r.id)).toEqual(["r1"]);
  });

  it("忙时无末尾回合则新建 live 回合;无流式正文时 active", () => {
    const views = buildTimeline(
      session({ busy: true, messages: [msg({ id: "u1", role: "user", content: "问" })] }),
    );
    expect(views).toHaveLength(2);
    const turn = views[1] as TurnView;
    expect(turn.key).toBe("1:live");
    expect(turn.live).toBe(true);
    expect(turn.active).toBe(true);
  });

  it("流式思考进过程组并标记 thinkingStreaming;重试/压缩状态挂到 live 回合", () => {
    const retry = { attempt: 1, maxAttempts: 3, delayMs: 5000, scheduledAt: 1000, message: "m" };
    const views = buildTimeline(
      session({
        busy: true,
        streamingThinking: "思考中",
        retry,
        compacting: true,
        messages: [],
      }),
    );
    const turn = views[0] as TurnView;
    expect(turn.groups[0]).toMatchObject({ thinking: "思考中", thinkingStreaming: true });
    expect(turn.retry).toEqual(retry);
    expect(turn.compacting).toBe(true);
  });
});

describe("retryRemainingSeconds", () => {
  it("无重试返回 0;倒计时向上取整且不为负", () => {
    expect(retryRemainingSeconds(null, 0)).toBe(0);
    const retry = { attempt: 1, maxAttempts: 3, delayMs: 5000, scheduledAt: 1000, message: "m" };
    expect(retryRemainingSeconds(retry, 1000)).toBe(5);
    expect(retryRemainingSeconds(retry, 3100)).toBe(3);
    expect(retryRemainingSeconds(retry, 99000)).toBe(0);
  });
});
