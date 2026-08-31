import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import type { ChatEvent, ChatUsageSummary } from "@/lib/chat";
import { useChatStore } from "@/stores/chat";

/** 捕获每次 sendChatMessage 的 onEvent 与 promise 控制柄,按项目路径索引 */
const chatHarness = vi.hoisted(() => {
  type Pending = {
    resolve: (usage: ChatUsageSummary | null) => void;
    reject: (error: unknown) => void;
    onEvent: (event: ChatEvent) => void;
  };
  const pending = new Map<string, Pending>();
  return { pending };
});

vi.mock("@/lib/chat", () => ({
  sendChatMessage: vi.fn(
    (project: { path: string }, params: { message: string; onEvent: (event: ChatEvent) => void }) =>
      new Promise<ChatUsageSummary | null>((resolve, reject) => {
        chatHarness.pending.set(project.path, {
          resolve,
          reject,
          onEvent: params.onEvent,
        });
      }),
  ),
  abortChat: vi.fn(async () => {}),
  newChatSession: vi.fn(async () => {}),
}));

const USAGE: ChatUsageSummary = {
  inputTokens: 100,
  outputTokens: 20,
  totalTokens: 120,
  cachedTokens: 10,
  costTotal: null,
  contextTokens: 120,
};

function emit(path: string, event: ChatEvent) {
  chatHarness.pending.get(path)?.onEvent(event);
}

function finish(path: string, usage: ChatUsageSummary | null = USAGE) {
  chatHarness.pending.get(path)?.resolve(usage);
}

function fail(path: string, error: unknown) {
  chatHarness.pending.get(path)?.reject(error);
}

const PATH = "D:\\repos\\demo";
const PROJECT = { path: PATH, name: "demo" };

describe("chat store", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    chatHarness.pending.clear();
    setActivePinia(createPinia());
  });

  it("textDelta 累积 streamingText,toolCall/toolResult 写入 toolRuns", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "这个项目是做什么的");

    emit(PATH, { kind: "textDelta", delta: "这是一" });
    emit(PATH, { kind: "toolCall", id: "t1", name: "read_file", args: { path: "a.rs" } });
    emit(PATH, { kind: "textDelta", delta: "个项目" });

    const session = store.sessions[PATH];
    expect(session.streamingText).toBe("这是一个项目");
    expect(session.busy).toBe(true);
    expect(session.phase).toBe("streaming");
    expect(session.toolRuns.t1).toEqual({
      name: "read_file",
      args: { path: "a.rs" },
      ok: null,
      summary: "",
    });
    expect(session.pendingToolRunIds).toEqual(["t1"]);

    emit(PATH, { kind: "toolResult", id: "t1", ok: true, summary: "10 行" });
    expect(session.toolRuns.t1).toMatchObject({ ok: true, summary: "10 行" });

    finish(PATH, null);
    await expect(run).resolves.toBe(true);
  });

  it("turnEnd 把 streamingText 固化为 assistant 消息并清空,done 结束整轮", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "总结结构");
    emit(PATH, { kind: "textDelta", delta: "前端 Vue" });
    emit(PATH, { kind: "toolCall", id: "t1", name: "list_files", args: {} });
    emit(PATH, { kind: "toolResult", id: "t1", ok: true, summary: "ok" });
    emit(PATH, { kind: "textDelta", delta: " + Rust" });
    emit(PATH, { kind: "turnEnd", contextTokens: 120 });

    const session = store.sessions[PATH];
    expect(session.streamingText).toBe("");
    expect(session.pendingToolRunIds).toEqual([]);
    expect(session.messages).toHaveLength(2);
    expect(session.messages[0]).toMatchObject({ role: "user", content: "总结结构" });
    expect(session.messages[1]).toMatchObject({
      role: "assistant",
      content: "前端 Vue + Rust",
      toolRunIds: ["t1"],
    });

    finish(PATH, USAGE);
    await run;
    expect(session.busy).toBe(false);
    expect(session.phase).toBe("idle");
    expect(session.lastUsage).toEqual(USAGE);
  });

  it("retryScheduled 丢弃失败 attempt 残片并在下一 attempt 开始时清理状态", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "重试一下");
    emit(PATH, { kind: "textDelta", delta: "失败前的部分内容" });
    emit(PATH, { kind: "toolCall", id: "failed-tool", name: "read_file", args: {} });
    emit(PATH, {
      kind: "retryScheduled",
      attempt: 1,
      maxAttempts: 3,
      delayMs: 2_000,
      message: "429: rate limited",
    });

    const session = store.sessions[PATH];
    expect(session.streamingText).toBe("");
    expect(session.pendingToolRunIds).toEqual([]);
    expect(session.retry).toMatchObject({
      attempt: 1,
      maxAttempts: 3,
      delayMs: 2_000,
      message: "429: rate limited",
    });

    emit(PATH, { kind: "retryStarted", attempt: 1, maxAttempts: 3 });
    expect(session.retry).toBeNull();
    emit(PATH, { kind: "textDelta", delta: "重试成功" });
    emit(PATH, { kind: "turnEnd", contextTokens: 130 });
    emit(PATH, { kind: "done", usage: USAGE });
    finish(PATH, USAGE);
    await run;

    expect(session.messages).toHaveLength(2);
    expect(session.messages[1]).toMatchObject({ role: "assistant", content: "重试成功" });
    expect(session.phase).toBe("idle");
  });

  it("最终 error 会清理 retry 状态", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "持续失败");
    emit(PATH, {
      kind: "retryScheduled",
      attempt: 3,
      maxAttempts: 3,
      delayMs: 8_000,
      message: "503",
    });
    emit(PATH, { kind: "error", code: "ai_request_failed", message: "503 service unavailable" });
    finish(PATH, null);
    await run;

    const session = store.sessions[PATH];
    expect(session.retry).toBeNull();
    expect(session.phase).toBe("error");
    expect(session.messages).toHaveLength(1);
  });

  it("忙时拒绝再次发送,不重复调用后端", async () => {
    const store = useChatStore();
    const first = store.send(PATH, PROJECT, "第一条");
    const accepted = await store.send(PATH, PROJECT, "第二条");

    expect(accepted).toBe(false);
    expect(chatHarness.pending.size).toBe(1);
    expect(store.sessions[PATH].messages).toHaveLength(1);

    finish(PATH);
    await first;
  });

  it("上一轮结束后清理在途记录,后续发送不被永久拒绝", async () => {
    const store = useChatStore();
    const first = store.send(PATH, PROJECT, "第一条");
    finish(PATH);
    await first;

    const second = store.send(PATH, PROJECT, "第二条");
    expect(second).not.toBeNull();
    emit(PATH, { kind: "textDelta", delta: "第二条的回答" });
    finish(PATH);
    await expect(second).resolves.toBe(true);

    const session = store.sessions[PATH];
    // 第一轮无 turnEnd/残余文本,只固化 user 消息;第二轮补齐 user + assistant
    expect(session.messages).toHaveLength(3);
    expect(session.messages[1]).toMatchObject({ role: "user", content: "第二条" });
    expect(session.messages[2]).toMatchObject({ role: "assistant", content: "第二条的回答" });
  });

  it("error 事件映射 chat_busy 前缀并进入 error 阶段", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "问题");
    emit(PATH, { kind: "error", code: "ai_request_failed", message: "chat_busy: 上一条还在跑" });
    finish(PATH, null);
    await run;

    const session = store.sessions[PATH];
    expect(session.error).toContain("稍候");
    expect(session.phase).toBe("error");
    expect(session.busy).toBe(false);
  });

  it("ai_not_configured 拒绝映射为配置引导文案", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "问题");
    fail(
      PATH,
      Object.assign(new Error("请先在设置页配置 AI 的 API Key"), {
        cause: { code: "ai_not_configured", message: "请先在设置页配置 AI 的 API Key" },
      }),
    );
    await run;

    const session = store.sessions[PATH];
    expect(session.error).toBe("AI 尚未配置,请先在设置页完成配置");
    expect(session.phase).toBe("error");
    expect(session.busy).toBe(false);
  });

  it("中止后 send 的 finally 固化残余流式文本为 partial 消息", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "写长一点");
    emit(PATH, { kind: "textDelta", delta: "写到一半" });
    store.abort(PATH);
    finish(PATH, null);
    await run;

    const session = store.sessions[PATH];
    expect(session.busy).toBe(false);
    expect(session.messages).toHaveLength(2);
    expect(session.messages[1]).toMatchObject({
      role: "assistant",
      content: "写到一半",
      partial: true,
    });
    expect(session.streamingText).toBe("");
  });

  it("newSession 清空前端会话并调用后端清理", async () => {
    const store = useChatStore();
    const run = store.send(PATH, PROJECT, "第一条");
    emit(PATH, { kind: "textDelta", delta: "回答" });
    emit(PATH, { kind: "turnEnd", contextTokens: 120 });
    finish(PATH);
    await run;

    await store.newSession(PATH);

    const session = store.sessions[PATH];
    expect(session.messages).toHaveLength(0);
    expect(session.toolRuns).toEqual({});
    expect(session.phase).toBe("idle");
    expect(session.error).toBeNull();
    const { newChatSession } = await import("@/lib/chat");
    expect(vi.mocked(newChatSession)).toHaveBeenCalledWith(PATH);
  });
});
