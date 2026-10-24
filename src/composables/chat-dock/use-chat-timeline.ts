import { computed, type ComputedRef } from "vue";
import { useNow } from "@vueuse/core";
import type { ChatMessage, ChatProcessGroup, ChatToolRun } from "@/lib/chat";
import type { ChatRetryState, ChatSessionState } from "@/stores/chat";

// ── 消息渲染:按「用户消息 / assistant 回合」归组 ──────────────────
// 与工具调用统一收进 ChatTurnProcess 折叠块;忙时流式过程/正文作为独立的
// live 回合追加在时间线末尾(不并入已提交回合),正文开始输出后过程块随
// active 翻转整体自动收起,回答之上只留一行摘要。

export interface TurnView {
  kind: "turn";
  key: string;
  groups: ChatProcessGroup[];
  contents: string[];
  /** 最后一段正文的固化时间(Unix 毫秒),未知时不展示 */
  completedAt: number | null;
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

/** 流式回合的固定 key(整个会话同时最多一个) */
export const LIVE_TURN_KEY = "live";

function resolveRuns(runIds: string[], toolRuns: Record<string, ChatToolRun>) {
  return runIds.map((id) => toolRuns[id]).filter((run) => run != null);
}

function emptyTurn(key: string): TurnView {
  return {
    kind: "turn",
    key,
    groups: [],
    contents: [],
    completedAt: null,
    active: false,
    live: false,
    streamingText: "",
    retry: null,
    compacting: false,
  };
}

/** 旧历史未保存 createdAt,但消息 ID 中包含当时的毫秒时间戳。 */
function messageTimestamp(message: { id: string; createdAt?: number }): number | null {
  const timestamp = message.createdAt ?? Number(/^chat-msg-(\d+)-/.exec(message.id)?.[1]);
  return Number.isFinite(timestamp) && timestamp > 0 && timestamp <= 8.64e15 ? timestamp : null;
}

// ── 已提交视图记忆化 ─────────────────────────────────────────────
// 已提交消息不可变,toolRuns 替换时未变更的 run 保留原引用,因此视图可以
// 以「消息 + 引用的 run」的对象引用集合为依赖做缓存:依赖全等则复用同一个
// 视图对象,流式期间下游组件因 props 未变而不重绘。

interface ViewCacheEntry {
  deps: unknown[];
  view: TimelineView;
}

export type ViewCache = Map<string, ViewCacheEntry>;

const VIEW_CACHE_LIMIT = 512;

function depsEqual(a: unknown[], b: unknown[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function cachedView(
  cache: ViewCache | undefined,
  key: string,
  deps: unknown[],
  build: () => TimelineView,
): TimelineView {
  if (!cache) return build();
  const hit = cache.get(key);
  if (hit && depsEqual(hit.deps, deps)) return hit.view;
  const view = build();
  // 会话切换/截断遗留的陈旧条目无害(消息 id 不复用),超限时整体清空防泄漏
  if (cache.size >= VIEW_CACHE_LIMIT) cache.clear();
  cache.set(key, { deps, view });
  return view;
}

interface TurnPart {
  message: ChatMessage;
  runs: ChatToolRun[];
}

function buildTurnView(key: string, parts: TurnPart[]): TurnView {
  const view = emptyTurn(key);
  for (const { message, runs } of parts) {
    if (message.thinking || runs.length > 0) {
      view.groups.push({ thinking: message.thinking, runs });
    }
    // 纯空白的正文(工具调用轮常夹带 "\n\n")不进渲染列表,否则空段落
    // 逐个叠加 flex gap,折叠块与正文之间会出现大段空白
    if (message.content.trim()) {
      view.contents.push(message.content);
      view.completedAt = messageTimestamp(message);
    }
  }
  return view;
}

/** 已提交消息 → 时间线视图(不含忙时的 live 回合);传 cache 时按依赖记忆化。 */
export function buildCommittedTimeline(s: ChatSessionState, cache?: ViewCache): TimelineView[] {
  const views: TimelineView[] = [];
  // 仅最后一条用户提问可编辑重发;忙时禁用(编辑会截断后端回合,与在途请求冲突)
  let lastUserId: string | null = null;
  for (let i = s.messages.length - 1; i >= 0; i--) {
    if (s.messages[i].role === "user") {
      lastUserId = s.messages[i].id;
      break;
    }
  }
  // 归并中的回合:连续 assistant 消息收集到同一回合,回合结束时一次性产出视图
  let parts: TurnPart[] = [];
  let turnKey = "";
  const flushTurn = () => {
    if (parts.length === 0) return;
    const key = turnKey;
    const captured = parts;
    parts = [];
    views.push(
      cachedView(
        cache,
        key,
        [...captured.map((part) => part.message), ...captured.flatMap((part) => part.runs)],
        () => buildTurnView(key, captured),
      ),
    );
  };
  for (const message of s.messages) {
    const compaction = message.compaction;
    if (compaction) {
      flushTurn();
      const { tokensBefore, tokensAfter } = compaction;
      views.push(
        cachedView(
          cache,
          message.id,
          [message],
          () => ({ kind: "compaction", key: message.id, tokensBefore, tokensAfter }),
        ),
      );
      continue;
    }
    if (message.role === "user") {
      flushTurn();
      const editable = !s.busy && message.id === lastUserId;
      views.push(
        cachedView(
          cache,
          message.id,
          [message, editable],
          () => ({ kind: "user", key: message.id, content: message.content, editable }),
        ),
      );
      continue;
    }
    if (parts.length === 0) turnKey = message.id;
    parts.push({ message, runs: resolveRuns(message.toolRunIds, s.toolRuns) });
  }
  flushTurn();
  return views;
}

/** 忙时的流式回合(独立于已提交回合);空闲返回 null。 */
export function buildLiveTurn(s: ChatSessionState): TurnView | null {
  if (!s.busy) return null;
  const turn = emptyTurn(LIVE_TURN_KEY);
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
  return turn;
}

/** 完整时间线(已提交视图 + 忙时 live 回合);无缓存的一次性版本。 */
export function buildTimeline(s: ChatSessionState): TimelineView[] {
  const live = buildLiveTurn(s);
  const base = buildCommittedTimeline(s);
  return live ? [...base, live] : base;
}

/**
 * 会话消息 → 时间线视图 + 重试倒计时(250ms tick 驱动秒数递减)。
 * 已提交视图与流式状态拆成两个 computed:textDelta 等流式增量只命中
 * liveTurn,base 依赖的 messages/toolRuns/busy 不变即不重算。
 */
export function useChatTimeline(session: ComputedRef<ChatSessionState>) {
  const cache: ViewCache = new Map();
  const base = computed(() => buildCommittedTimeline(session.value, cache));
  const liveTurn = computed(() => buildLiveTurn(session.value));
  const timeline = computed(() => (liveTurn.value ? [...base.value, liveTurn.value] : base.value));
  const retryClock = useNow({ interval: 250 });
  const retrySeconds = computed(() =>
    retryRemainingSeconds(session.value.retry, retryClock.value.getTime()),
  );
  return { timeline, retrySeconds };
}

/** 重试倒计时剩余秒数(0 下限)。 */
export function retryRemainingSeconds(retry: ChatRetryState | null, now: number): number {
  if (!retry) return 0;
  const elapsed = now - retry.scheduledAt;
  return Math.max(0, Math.ceil((retry.delayMs - elapsed) / 1000));
}
