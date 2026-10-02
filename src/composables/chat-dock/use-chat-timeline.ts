import { computed, type ComputedRef } from "vue";
import { useNow } from "@vueuse/core";
import type { ChatProcessGroup, ChatToolRun } from "@/lib/chat";
import type { ChatRetryState, ChatSessionState } from "@/stores/chat";

// ── 消息渲染:按「用户消息 / assistant 回合」归组 ──────────────────
// 与工具调用统一收进 ChatTurnProcess 折叠块;忙时流式过程/正文并入末尾回合,
// 最终正文开始输出后过程块随 active 翻转整体自动收起,回答之上只留一行摘要。

export interface TurnView {
  kind: "turn";
  key: string;
  groups: ChatProcessGroup[];
  contents: string[];
  /** 过程仍在产出(本轮正文尚未开始):驱动折叠块保持展开跟随流式 */
  active: boolean;
  /** 回合仍在流式产出:附流式正文 / 重试等待 / Loader */
  live: boolean;
  streamingText: string;
  retry: ChatRetryState | null;
  /** 自动压缩进行中(替代 Loader 的状态行) */
  compacting: boolean;
}

export type TimelineView =
  | { kind: "user"; key: string; content: string; editable: boolean }
  | { kind: "compaction"; key: string; tokensBefore: number; tokensAfter: number }
  | TurnView;

function resolveRuns(runIds: string[], toolRuns: Record<string, ChatToolRun>) {
  return runIds.map((id) => toolRuns[id]).filter((run) => run != null);
}

function emptyTurn(key: string): TurnView {
  return {
    kind: "turn",
    key,
    groups: [],
    contents: [],
    active: false,
    live: false,
    streamingText: "",
    retry: null,
    compacting: false,
  };
}

export function buildTimeline(s: ChatSessionState): TimelineView[] {
  const views: TimelineView[] = [];
  let turn: TurnView | null = null;
  // 仅最后一条用户提问可编辑重发;忙时禁用(编辑会截断后端回合,与在途请求冲突)
  let lastUserId: string | null = null;
  for (let i = s.messages.length - 1; i >= 0; i--) {
    if (s.messages[i].role === "user") {
      lastUserId = s.messages[i].id;
      break;
    }
  }
  for (const message of s.messages) {
    if (message.compaction) {
      turn = null;
      views.push({
        kind: "compaction",
        key: message.id,
        tokensBefore: message.compaction.tokensBefore,
        tokensAfter: message.compaction.tokensAfter,
      });
      continue;
    }
    if (message.role === "user") {
      turn = null;
      views.push({
        kind: "user",
        key: message.id,
        content: message.content,
        editable: !s.busy && message.id === lastUserId,
      });
      continue;
    }
    if (!turn) {
      turn = emptyTurn(message.id);
      views.push(turn);
    }
    const runs = resolveRuns(message.toolRunIds, s.toolRuns);
    if (message.thinking || runs.length > 0) {
      turn.groups.push({ thinking: message.thinking, runs });
    }
    // 纯空白的正文(工具调用轮常夹带 "\n\n")不进渲染列表,否则空段落
    // 逐个叠加 flex gap,折叠块与正文之间会出现大段空白
    if (message.content.trim()) turn.contents.push(message.content);
  }
  // 忙时:流式状态并入末尾回合(没有则新建),整轮共用同一个折叠块
  if (s.busy) {
    if (!turn) {
      turn = emptyTurn(`${s.messages.length}:live`);
      views.push(turn);
    }
    turn.live = true;
    turn.streamingText = s.streamingText;
    turn.retry = s.retry;
    turn.compacting = s.compacting;
    turn.active = s.pendingToolRunIds.length > 0 || !s.streamingText.trim();
    const runs = resolveRuns(s.pendingToolRunIds, s.toolRuns);
    if (s.streamingThinking || runs.length > 0) {
      turn.groups.push({
        thinking: s.streamingThinking || undefined,
        thinkingStreaming: true,
        runs,
      });
    }
  }
  return views;
}

/** 重试倒计时剩余秒数(0 下限)。 */
export function retryRemainingSeconds(retry: ChatRetryState | null, now: number): number {
  if (!retry) return 0;
  const elapsed = now - retry.scheduledAt;
  return Math.max(0, Math.ceil((retry.delayMs - elapsed) / 1000));
}

/** 会话消息 → 时间线视图 + 重试倒计时(250ms  tick 驱动秒数递减)。 */
export function useChatTimeline(session: ComputedRef<ChatSessionState>) {
  const timeline = computed(() => buildTimeline(session.value));
  const retryClock = useNow({ interval: 250 });
  const retrySeconds = computed(() =>
    retryRemainingSeconds(session.value.retry, retryClock.value.getTime()),
  );
  return { timeline, retrySeconds };
}
