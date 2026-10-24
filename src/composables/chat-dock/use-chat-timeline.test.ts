import { describe, expect, it } from "vitest";
import type { ChatMessage, ChatToolRun } from "@/lib/chat";
import type { ChatSessionState } from "@/stores/chat";
import {
  buildCommittedTimeline,
  buildTimeline,
  retryRemainingSeconds,
  type TurnView,
} from "./use-chat-timeline";

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

  it("回答时间取最后一段正文,不被后续纯工具消息覆盖", () => {
    const views = buildTimeline(
      session({
        messages: [
          msg({ id: "a1", role: "assistant", content: "第一段", createdAt: 1000 }),
          msg({ id: "a2", role: "assistant", content: "第二段", createdAt: 2000 }),
          msg({ id: "a3", role: "assistant", thinking: "检查", createdAt: 3000 }),
          msg({ id: "u1", role: "user", content: "新问题" }),
          msg({ id: "a4", role: "assistant", content: "新回答", createdAt: 4000 }),
        ],
      }),
    );
    expect((views[0] as TurnView).completedAt).toBe(2000);
    expect((views[2] as TurnView).completedAt).toBe(4000);
  });

  it("旧历史从消息 ID 恢复时间,未知或无效时间不显示", () => {
    for (const [message, expected] of [
      [
        msg({ id: "chat-msg-1792200000000-abc", role: "assistant", content: "旧回答" }),
        1792200000000,
      ],
      [msg({ id: "a1", role: "assistant", content: "无时间" }), null],
      [msg({ id: "a2", role: "assistant", content: "坏时间", createdAt: NaN }), null],
      [msg({ id: "a3", role: "assistant", content: "超出范围", createdAt: 9e15 }), null],
    ] as const) {
      const turn = buildTimeline(session({ messages: [message] }))[0] as TurnView;
      expect(turn.completedAt).toBe(expected);
    }
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

  it("忙时流式状态作为独立 live 回合追加,不并入已提交回合", () => {
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
    expect(views).toHaveLength(3);
    // 已提交回合保持固化内容,不受流式影响
    const committed = views[1] as TurnView;
    expect(committed.key).toBe("a1");
    expect(committed.live).toBe(false);
    expect(committed.contents).toEqual(["已固化"]);
    // 流式正文/工具调用收在末尾的 live 回合
    const turn = views[2] as TurnView;
    expect(turn.key).toBe("live");
    expect(turn.live).toBe(true);
    expect(turn.streamingText).toBe("流式中");
    expect(turn.contents).toEqual([]);
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
    expect(turn.key).toBe("live");
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

describe("buildCommittedTimeline 记忆化", () => {
  it("依赖未变时复用同一视图对象;工具运行更新才重建对应回合", () => {
    const run: ChatToolRun = {
      id: "r1",
      name: "bash",
      args: {},
      ok: null,
      summary: "",
      permission: "pending",
    };
    const state = session({
      messages: [
        msg({ id: "u1", role: "user", content: "问" }),
        msg({ id: "a1", role: "assistant", content: "答", toolRunIds: ["r1"] }),
      ],
      toolRuns: { r1: run },
    });
    const cache = new Map();
    const first = buildCommittedTimeline(state, cache);
    // 全量重建后依赖全等:视图对象 identity 不变(流式期间不触发下游重绘的关键)
    expect(buildCommittedTimeline(state, cache)).toEqual(first);
    expect(buildCommittedTimeline(state, cache)[1]).toBe(first[1]);

    // run 对象被替换(权限状态流转):仅引用它的回合重建
    const resolved = { ...run, permission: "allowed" as const };
    const updated = buildCommittedTimeline(
      session({ ...state, toolRuns: { r1: resolved } }),
      cache,
    );
    expect(updated[0]).toBe(first[0]);
    expect(updated[1]).not.toBe(first[1]);
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
