import { Channel } from "@tauri-apps/api/core";
import { cmd } from "@/lib/tauri";

/**
 * 项目问答(chat)前后端桥:一条消息的完整工具循环由后端 Agent 驱动,
 * 前端只消费事件流并渲染。事件 tag(kind)与 Rust 侧 serde rename camelCase 对齐。
 */

/** 整轮工具循环的用量汇总(usage 为空 = 后端未返回统计) */
export interface ChatUsageSummary {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedTokens: number | null;
  costTotal: number | null;
  /** 当前上下文占用(最近 assistant 消息的 total_tokens;null = 暂无统计) */
  contextTokens: number | null;
}

/** 上下文构成估算(最近一次 LLM 请求,按系统提示词/工具定义/消息三部分 tiktoken 计量) */
export interface ChatContextBreakdown {
  systemPrompt: number;
  tools: number;
  messages: number;
}

/** chat_send 过程中经 Channel 推送的事件(与 Rust ChatEvent 一一对应) */
export type ChatEvent =
  | { kind: "textDelta"; delta: string }
  | { kind: "thinkingDelta"; delta: string }
  | { kind: "toolCall"; id: string; name: string; args: unknown }
  | { kind: "toolPermissionRequest"; id: string; name: string; args: unknown }
  | { kind: "toolResult"; id: string; ok: boolean; summary: string }
  | {
      kind: "turnEnd";
      contextTokens: number | null;
      breakdown?: ChatContextBreakdown | null;
    }
  | {
      kind: "retryScheduled";
      attempt: number;
      maxAttempts: number;
      delayMs: number;
      message: string;
    }
  | { kind: "retryStarted"; attempt: number; maxAttempts: number }
  | { kind: "done"; usage: ChatUsageSummary | null }
  | { kind: "error"; code: string; message: string };

export type ChatRole = "user" | "assistant";

/** 聊天消息(toolRunIds 指向 store 里同轮产生的工具调用记录,仅 assistant 携带) */
export interface ChatMessage {
  id: string;
  role: ChatRole;
  content: string;
  /** 该条消息的思考过程原文(reasoning 模型;无思考输出时缺省) */
  thinking?: string;
  /** 该条消息期间发生的工具调用 id 列表(按发生顺序) */
  toolRunIds: string[];
  /** 该条消息是中止/异常时的残缺回复(仅展示标记) */
  partial?: boolean;
}

/** 工具权限审批状态:null = 无需审批(all 档直接执行) */
export type ChatToolPermission = "pending" | "responding" | "allowed" | "denied";

/** 一次工具调用的展示状态(与 ChatEvent 的 toolCall/toolResult 通过 id 关联) */
export interface ChatToolRun {
  id: string;
  name: string;
  args: unknown;
  /** null = 仍在运行;true/false = 结果状态 */
  ok: boolean | null;
  summary: string;
  /** 权限审批状态(ask 档下需要确认的工具;null = 无需审批或已收尾) */
  permission: ChatToolPermission | null;
}

/** 回答过程折叠块里的单个轮次段:该轮思考原文与工具调用(按发生顺序) */
export interface ChatProcessGroup {
  thinking?: string;
  /** 思考仍在流式累积(仅进行中的轮次为 true,正文用 streaming 模式渲染) */
  thinkingStreaming?: boolean;
  runs: ChatToolRun[];
}

function chatRunId(): string {
  return `chat-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/** token 数紧凑格式:999 → "999"、1234 → "1.2k"、1048576 → "1M"(上下文占用展示用) */
export function formatTokenCount(tokens: number): string {
  if (!Number.isFinite(tokens) || tokens <= 0) return "0";
  if (tokens >= 1_000_000) {
    return `${(tokens / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
  }
  if (tokens >= 1_000) {
    return `${(tokens / 1_000).toFixed(1).replace(/\.0$/, "")}k`;
  }
  return String(Math.round(tokens));
}

/** AbortSignal → chat_abort 的取消绑定,参照 lib/ai.ts 的 bindCancellation 模式 */
function bindCancellation(id: string, signal?: AbortSignal): () => void {
  if (!signal) return () => {};
  const cancel = () => void cmd<void>("chat_abort", { runId: id }).catch(() => {});
  if (signal.aborted) cancel();
  else signal.addEventListener("abort", cancel, { once: true });
  return () => signal.removeEventListener("abort", cancel);
}

/**
 * 发送一条消息并等待整轮完成(工具循环在后端 Agent 内部)。
 * 流式事件经 params.onEvent 回调;整轮结束返回累计用量(取消/最终失败返回 null)。
 */
export async function sendChatMessage(
  project: { path: string; name: string },
  params: {
    message: string;
    onEvent: (event: ChatEvent) => void;
    signal?: AbortSignal;
  },
): Promise<ChatUsageSummary | null> {
  const runId = chatRunId();
  const unbind = bindCancellation(runId, params.signal);
  const channel = new Channel<ChatEvent>();
  channel.onmessage = params.onEvent;
  try {
    return await cmd<ChatUsageSummary | null>("chat_send", {
      runId,
      projectPath: project.path,
      projectName: project.name,
      message: params.message,
      onEvent: channel,
    });
  } finally {
    unbind();
  }
}

/** 中止当前运行(与 ai_cancel_run 后端等价,优先用本命令) */
export function abortChat(runId: string): Promise<void> {
  return cmd<void>("chat_abort", { runId });
}

/** 清空后端会话上下文(新会话) */
export function newChatSession(projectPath: string): Promise<void> {
  return cmd<void>("chat_new_session", { projectPath });
}

/** 回应工具权限请求:allow=true 允许本次执行,false 拒绝(后端仍会以 toolResult 收尾) */
export function respondToolPermission(
  projectPath: string,
  toolCallId: string,
  allow: boolean,
): Promise<boolean> {
  return cmd<boolean>("chat_tool_permission_respond", { projectPath, toolCallId, allow });
}
