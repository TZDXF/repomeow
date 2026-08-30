import { ref } from "vue";
import { defineStore } from "pinia";
import { i18n } from "@/i18n";
import {
  newChatSession,
  sendChatMessage,
  type ChatEvent,
  type ChatMessage,
  type ChatToolRun,
  type ChatUsageSummary,
} from "@/lib/chat";

export type ChatPhase = "idle" | "streaming" | "error";

/** 单个项目的会话状态(离开页面不清空,内存会话随应用退出丢弃) */
export interface ChatSessionState {
  messages: ChatMessage[];
  phase: ChatPhase;
  /** 当前回复的流式增量累积(turnEnd 时固化为 assistant 消息) */
  streamingText: string;
  /** 本轮尚待固化的工具调用 id(与 messages 里固化的 toolRunIds 区分) */
  pendingToolRunIds: string[];
  toolRuns: Record<string, ChatToolRun>;
  error: string | null;
  busy: boolean;
  /** 最近一次整轮完成的用量(done 时由 chat_send 返回值回填) */
  lastUsage: ChatUsageSummary | null;
  /** 当前上下文占用(turnEnd 逐轮更新;null = 暂无统计) */
  contextTokens: number | null;
}

interface ChatProject {
  path: string;
  name: string;
}

function defaultSession(): ChatSessionState {
  return {
    messages: [],
    phase: "idle",
    streamingText: "",
    pendingToolRunIds: [],
    toolRuns: {},
    error: null,
    busy: false,
    lastUsage: null,
    contextTokens: null,
  };
}

function messageId(): string {
  return `chat-msg-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

interface ChatErrorShape {
  code: string;
  message: string;
}

/** 从桥接错误(cmd 包装的 Error,cause 携带原始 AppError)中提取 code/message */
export function extractChatError(error: unknown): ChatErrorShape {
  const cause = (error as { cause?: unknown } | null)?.cause;
  if (cause && typeof cause === "object") {
    const raw = cause as { code?: unknown; message?: unknown };
    return {
      code: typeof raw.code === "string" ? raw.code : "",
      message: typeof raw.message === "string" ? raw.message : "",
    };
  }
  const code = (error as { code?: unknown } | null)?.code;
  return {
    code: typeof code === "string" ? code : "",
    message: error instanceof Error ? error.message : String(error),
  };
}

/** 错误码 → 用户可见文案(与 chat.errors.* i18n 词条对齐) */
export function friendlyChatError(code: string, message: string): string {
  if (code === "ai_not_configured") {
    return i18n.global.t("chat.errors.ai_not_configured");
  }
  if (code === "ai_request_failed") {
    if (message.startsWith("chat_busy")) {
      return i18n.global.t("chat.errors.chat_busy");
    }
    const base = i18n.global.t("chat.errors.ai_request_failed");
    return message ? `${base}\n${message}` : base;
  }
  if (message) {
    return message;
  }
  return i18n.global.t("chat.errors.generic");
}

export const useChatStore = defineStore("chat", () => {
  // key = clean 后的项目路径(与 ChatDock 传入的 project.path 一致)
  const sessions = ref<Record<string, ChatSessionState>>({});
  // 非响应式运行簿:AbortController 与在途 promise,仅作取消/会话清空的协调
  const controllers = new Map<string, AbortController>();
  const activeRuns = new Map<string, Promise<void>>();

  function ensureSession(path: string): ChatSessionState {
    const existing = sessions.value[path];
    if (existing) return existing;
    const created = defaultSession();
    sessions.value[path] = created;
    return created;
  }

  function onChatEvent(path: string, event: ChatEvent) {
    const session = sessions.value[path];
    if (!session) return;
    switch (event.kind) {
      case "textDelta":
        session.streamingText += event.delta;
        break;
      case "toolCall":
        session.toolRuns = {
          ...session.toolRuns,
          [event.id]: { name: event.name, args: event.args, ok: null, summary: "" },
        };
        session.pendingToolRunIds = [...session.pendingToolRunIds, event.id];
        break;
      case "toolResult": {
        const run = session.toolRuns[event.id];
        if (run) {
          session.toolRuns = {
            ...session.toolRuns,
            [event.id]: { ...run, ok: event.ok, summary: event.summary },
          };
        }
        break;
      }
      case "turnEnd": {
        const content = session.streamingText;
        const toolRunIds = [...session.pendingToolRunIds];
        // 纯工具轮(模型只调工具没说话)也固化,保留时间线
        if (content || toolRunIds.length > 0) {
          const message: ChatMessage = {
            id: messageId(),
            role: "assistant",
            content,
            toolRunIds,
          };
          session.messages = [...session.messages, message];
        }
        session.streamingText = "";
        session.pendingToolRunIds = [];
        if (event.contextTokens != null) {
          session.contextTokens = event.contextTokens;
        }
        break;
      }
      case "error":
        session.error = friendlyChatError(event.code, event.message);
        session.phase = "error";
        break;
      case "done":
        // 用量由 chat_send 的返回值回填;最终失败以先行到达的 error 事件为准
        break;
    }
  }

  /**
   * 发送一条消息。忙或消息为空时直接拒绝(返回 false,由 UI 出提示),
   * 不排队:聊天是强交互场景,静默排队会让「停止」语义变得混乱。
   */
  function send(path: string, project: ChatProject, text: string): Promise<boolean> {
    const trimmed = text.trim();
    if (!trimmed) return Promise.resolve(false);
    if (activeRuns.has(path)) return Promise.resolve(false);

    const controller = new AbortController();
    controllers.set(path, controller);
    const session = ensureSession(path);
    session.error = null;
    session.busy = true;
    session.phase = "streaming";
    session.streamingText = "";
    session.pendingToolRunIds = [];
    session.lastUsage = null;
    session.messages = [
      ...session.messages,
      { id: messageId(), role: "user", content: trimmed, toolRunIds: [] },
    ];

    const run = (async () => {
      try {
        session.lastUsage = await sendChatMessage(
          { path: project.path, name: project.name },
          {
            message: trimmed,
            onEvent: (event) => onChatEvent(path, event),
            signal: controller.signal,
          },
        );
      } catch (error) {
        const { code, message } = extractChatError(error);
        session.error = friendlyChatError(code, message);
        session.phase = "error";
      } finally {
        // 中止/异常退出时把残余流式文本固化为残缺回复,避免丢字
        if (session.streamingText) {
          const message: ChatMessage = {
            id: messageId(),
            role: "assistant",
            content: session.streamingText,
            toolRunIds: [...session.pendingToolRunIds],
            partial: true,
          };
          session.messages = [...session.messages, message];
        }
        session.pendingToolRunIds = [];
        session.streamingText = "";
        session.busy = false;
        controllers.delete(path);
        if (session.phase === "streaming") {
          session.phase = "idle";
        }
      }
    })();

    activeRuns.set(path, run);
    // run 内部已 try/catch,不会 reject;finally 里按原 run 引用清理,
    // 若用 run.finally() 的返回值入表,则永远不等于 run,条目无法删除,
    // 该项目的后续发送会被 busy 守卫永久拒绝。
    void run.finally(() => {
      if (activeRuns.get(path) === run) activeRuns.delete(path);
    });
    return run.then(() => true);
  }

  /** 中止当前运行:经 AbortSignal → chat_abort;残余状态由 send 的 finally 收尾 */
  function abort(path: string) {
    controllers.get(path)?.abort();
  }

  /** 新会话:忙时先中止并等待落地,再清空前端内存与后端会话 */
  async function newSession(path: string) {
    const controller = controllers.get(path);
    if (controller) {
      controller.abort();
      const run = activeRuns.get(path);
      if (run) {
        await run.catch(() => {});
      }
    }
    sessions.value[path] = defaultSession();
    try {
      await newChatSession(path);
    } catch {
      /* 后端清理失败不阻断前端清空:下一次发送会以新会话语义重试 */
    }
  }

  return {
    sessions,
    ensureSession,
    send,
    abort,
    newSession,
  };
});
