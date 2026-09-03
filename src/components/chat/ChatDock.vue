<script setup lang="ts">
import { computed, nextTick, onMounted, ref, useTemplateRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import {
  Bot,
  Brain,
  Copy,
  Eye,
  Maximize2,
  MessageCircleMore,
  Minimize2,
  Pencil,
  RotateCcw,
  Square,
  TriangleAlert,
  Wrench,
  X,
} from "@lucide/vue";
import { useEventListener, useNow } from "@vueuse/core";
import {
  Context,
  ContextBreakdownUsage,
  ContextCacheHitRate,
  ContextContent,
  ContextContentBody,
  ContextContentFooter,
  ContextContentHeader,
  ContextTrigger,
  type ContextUsage,
} from "@/components/ai-elements/context";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Loader } from "@/components/ai-elements/loader";
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent,
  MessageResponse,
} from "@/components/ai-elements/message";
import { ModelSelector, type ModelSelectorGroup } from "@/components/ai-elements/model-selector";
import {
  PromptInput,
  PromptInputBody,
  PromptInputButton,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  type ChatStatus,
  type PromptInputMessage,
} from "@/components/ai-elements/prompt-input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Button } from "@/components/ui/button";
import ChatTurnProcess from "@/components/chat/ChatTurnProcess.vue";
import { CHAT_THINKING_LEVELS, type ChatPermission, type ChatThinkingLevel } from "@/lib/ai-config";
import type { ChatProcessGroup, ChatToolRun } from "@/lib/chat";
import { copyToClipboard } from "@/lib/utils";
import { useAiConfigStore } from "@/stores/ai-config";
import { useChatStore, type ChatRetryState } from "@/stores/chat";
import type { Project } from "@/types";

const props = defineProps<{ project: Project }>();

const { t, locale } = useI18n();
const router = useRouter();
const chat = useChatStore();
const aiConfig = useAiConfigStore();

// --- AI 配置就绪(多厂商配置 ai-config.json;chat 模型有效且厂商 baseUrl/apiKey 齐) ---
onMounted(() => {
  void aiConfig.ensureLoaded();
});
const aiReady = computed(() => aiConfig.chatReady);

// --- 会话状态(按项目路径隔离,离开页面不清空) ---
const session = computed(() => chat.ensureSession(props.project.path));

const suggestions = computed(() => [
  t("chat.suggestion1"),
  t("chat.suggestion2"),
  t("chat.suggestion3"),
]);

const isEmpty = computed(() => session.value.messages.length === 0 && !session.value.busy);

// --- 开合:收起为圆形入口,展开为右下角固定面板;ESC 或关闭钮收起 ---
const open = ref(false);

useEventListener(window, "keydown", (event: KeyboardEvent) => {
  if (open.value && event.key === "Escape") {
    event.stopPropagation();
    open.value = false;
  }
});

function toggleOpen() {
  open.value = !open.value;
}

// --- 放大/还原与边缘拖拽:面板尺寸由响应式宽高驱动;默认 500x640,放大铺满高度 ---
// 高度让出自绘标题栏(TitleBar.vue h-9 = 2.25rem,z-60 盖在浮层 z-50 之上,
// 约定同 lib/popper.ts):底部边距 1rem + 标题栏下间隙 1rem + 标题栏 2.25rem
const PANEL_DEFAULT_WIDTH = 500;
const PANEL_DEFAULT_HEIGHT = 640;
const PANEL_EXPANDED_WIDTH = 720;
const PANEL_FALLBACK_MIN_WIDTH = 420;
const PANEL_MIN_HEIGHT = 320;
const PANEL_MARGIN = 68; // 2rem + 2.25rem
// 底栏固定占位:发送/停止槽 32 + 上下文圆圈约 28 + footer 横向 padding 16
// + 间隙 8 + 输入区 p-3 24 + 边框 2 ≈ 110;实测工具组 scrollWidth 后加上它
const PANEL_TOOLS_OVERHEAD = 110;

const expanded = ref(false);
const panelWidth = ref(PANEL_DEFAULT_WIDTH);
const panelHeight = ref(PANEL_DEFAULT_HEIGHT);

const maxPanelWidth = () => window.innerWidth - 32;
const maxPanelHeight = () => window.innerHeight - PANEL_MARGIN;

function toggleExpanded() {
  expanded.value = !expanded.value;
  if (expanded.value) {
    panelWidth.value = Math.min(PANEL_EXPANDED_WIDTH, maxPanelWidth());
    panelHeight.value = maxPanelHeight();
  } else {
    panelWidth.value = PANEL_DEFAULT_WIDTH;
    panelHeight.value = PANEL_DEFAULT_HEIGHT;
  }
}

const panelStyle = computed(() => ({
  width: `${panelWidth.value}px`,
  height: `${panelHeight.value}px`,
  minWidth: `${minPanelWidth.value}px`,
  maxWidth: "calc(100vw - 2rem)",
  maxHeight: "calc(100vh - 2rem - 2.25rem)",
}));

// 最小宽度实测:临时把工具组脱离 flex 布局按内容量出自然单行宽度,保证底栏
// (权限/模型/思考强度 + 上下文圆圈 + 发送钮)永不换行;标签随模型/语言变化,
// 打开面板或相关值变化时重测
const toolsRef = useTemplateRef<{ $el: HTMLElement } | null>("tools");
const minPanelWidth = ref(PANEL_FALLBACK_MIN_WIDTH);

async function measureMinPanelWidth() {
  await nextTick();
  const el = toolsRef.value?.$el;
  if (!el) return;
  // 不能靠叠 flex-nowrap 读 scrollWidth:Tailwind v4 里 .flex-wrap 层叠序晚于
  // .flex-nowrap(叠加无效),且 flex-1(basis:0%)下 scrollWidth 恒等于
  // clientWidth——量到的是当前分得的空间而非内容宽度,每次重测都会把
  // 「当前宽度 + 常量开销」写成新 min-width,切换下拉即不断变宽
  el.style.flex = "none";
  el.style.width = "max-content";
  const toolsWidth = el.getBoundingClientRect().width;
  el.style.flex = "";
  el.style.width = "";
  minPanelWidth.value = Math.max(
    PANEL_FALLBACK_MIN_WIDTH,
    Math.ceil(toolsWidth) + PANEL_TOOLS_OVERHEAD,
  );
}

watch(
  [open, () => aiConfig.chatModelValue, () => aiConfig.chatPermission, () => locale.value],
  () => {
    if (open.value) void measureMinPanelWidth();
  },
);

// 上边/左边/左上角拖拽:面板锚定右下角,拖左边加宽、拖上边加高;
// 拖拽中禁用尺寸过渡,避免 0.25s 过渡滞后于指针
const resizing = ref(false);

function startResize(axis: "x" | "y" | "both", event: PointerEvent) {
  if (event.button !== 0) return;
  event.preventDefault();
  const handle = event.currentTarget as HTMLElement;
  const startX = event.clientX;
  const startY = event.clientY;
  const startWidth = panelWidth.value;
  const startHeight = panelHeight.value;
  resizing.value = true;
  handle.setPointerCapture(event.pointerId);
  const onMove = (e: PointerEvent) => {
    expanded.value = false;
    if (axis !== "y") {
      panelWidth.value = Math.min(
        Math.max(startWidth + (startX - e.clientX), minPanelWidth.value),
        maxPanelWidth(),
      );
    }
    if (axis !== "x") {
      panelHeight.value = Math.min(
        Math.max(startHeight + (startY - e.clientY), PANEL_MIN_HEIGHT),
        maxPanelHeight(),
      );
    }
  };
  const onUp = () => {
    resizing.value = false;
    handle.removeEventListener("pointermove", onMove);
    handle.removeEventListener("pointerup", onUp);
    handle.removeEventListener("pointercancel", onUp);
  };
  handle.addEventListener("pointermove", onMove);
  handle.addEventListener("pointerup", onUp);
  handle.addEventListener("pointercancel", onUp);
}

// --- 发送 / 停止 / 新会话 ---
function onSubmit(message: PromptInputMessage) {
  sendText(message.text);
}

function sendText(text: string) {
  const trimmed = text.trim();
  if (!trimmed || !aiReady.value || session.value.busy) return;
  cancelEdit();
  void chat.send(props.project.path, props.project, trimmed);
}

function abort() {
  chat.abort(props.project.path);
}

// --- 编辑上一条提问:最后一条用户消息悬停出现编辑钮,气泡内联编辑,
// Enter 确认(截掉该提问的回答回合后以新文本重发),Esc 取消 ---
const editingKey = ref<string | null>(null);
const editText = ref("");
const editTextareaRef = useTemplateRef<HTMLTextAreaElement>("editTextarea");

const editRows = computed(() => Math.min(10, Math.max(2, editText.value.split("\n").length)));

function startEdit(view: { key: string; content: string }) {
  editingKey.value = view.key;
  editText.value = view.content;
  void nextTick(() => {
    const el = editTextareaRef.value;
    if (el) {
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
    }
  });
}

function cancelEdit() {
  editingKey.value = null;
  editText.value = "";
}

// --- 复制:提问复制原文;回答复制该回合的全部正文段(流式中不显示,
// 避免复制到残缺文本)。成功/失败提示由 copyToClipboard 统一 toast ---
function copyText(text: string) {
  void copyToClipboard(text);
}

function copyTurn(view: TurnView) {
  copyText(view.contents.join("\n\n"));
}

function confirmEdit() {
  if (editingKey.value === null) return;
  const text = editText.value.trim();
  if (!text || !aiReady.value || session.value.busy) return;
  cancelEdit();
  void chat.editLastUserMessage(props.project.path, props.project, text);
}

// 发送/停止共用一个位置(参照 ai-elements chatbot 示例):状态驱动提交按钮图标,
// 等待首个 token 转圈、流式中显示方块;忙时提交钮隐藏但保留在 DOM 且 disabled,
// 使输入框 Enter 守卫(button[type=submit] disabled 则不提交)继续生效、不误清已输入文本
const submitStatus = computed<ChatStatus>(() => {
  if (session.value.busy) return session.value.streamingText ? "streaming" : "submitted";
  return session.value.error ? "error" : "ready";
});

function startNewSession() {
  // 忙时直接清:store 内部先中止在途请求,等待落地后重置前后端会话
  cancelEdit();
  void chat.newSession(props.project.path);
}

// ── 底栏控制区:模型 / 思考强度 / 权限 / 上下文占用 ──────────────────
// 偏好写入 ai-config.json 后 Rust 侧每次 chat_send 前热读;回答回合进行中
// 一律禁用切换,避免在途 LLM 调用与界面状态不一致。

const modelGroups = computed<ModelSelectorGroup[]>(() => {
  const config = aiConfig.config;
  if (!config) return [];
  return Object.entries(config.providers).map(([providerId, provider]) => ({
    providerId,
    providerName: provider.name || providerId,
    models: provider.models,
  }));
});

const thinkingDisabled = computed(() => !aiConfig.chatModel?.model.reasoning);
const thinkingTitle = computed(() =>
  thinkingDisabled.value ? t("chat.thinkingUnsupported") : t("chat.thinking"),
);

const permissionTitle = computed(() =>
  aiConfig.chatPermission === "all" ? t("chat.permission.all") : t("chat.permission.ask"),
);

/** 偏好写入失败统一 toast(落盘失败时 store 已回读后端真实状态) */
function applyPref(action: () => Promise<void>) {
  return action().catch((error: unknown) => {
    toast.error(error instanceof Error ? error.message : String(error));
  });
}

function onModelChange(value: string) {
  void applyPref(() => aiConfig.setChatModelValue(value));
}

function onThinkingChange(level: unknown) {
  if (typeof level !== "string") return;
  void applyPref(() => aiConfig.setChatThinking(level as ChatThinkingLevel));
}

function onPermissionChange(value: unknown) {
  if (value !== "all" && value !== "ask") return;
  void applyPref(() => aiConfig.setChatPermission(value as ChatPermission));
}

/** ChatToolCard 冒泡的工具权限请求:经 store 回应后端;失败 toast 提示可重试 */
function onToolPermissionRespond(payload: { id: string; allow: boolean }) {
  void chat.respondToolPermission(props.project.path, payload.id, payload.allow).then((ok) => {
    if (!ok) toast.error(t("chat.toolPermission.respondFailed"));
  });
}

// 上下文占用:窗口来自所选模型元数据;明细来自最近一次整轮用量
const contextWindow = computed(() => {
  const window = aiConfig.chatModel?.model.contextWindow;
  return window && window > 0 ? window : null;
});

// 上一轮用量明细(context 弹层的 Input/Output/Cache 行);成本费率取自模型元数据
const contextUsage = computed<ContextUsage | undefined>(() => {
  const usage = session.value.lastUsage;
  if (!usage) return undefined;
  return {
    inputTokens: usage.inputTokens,
    outputTokens: usage.outputTokens,
    cachedInputTokens: usage.cachedTokens ?? undefined,
  };
});

const contextCost = computed(() => aiConfig.chatModel?.model.cost ?? null);

// 平均缓存命中率:各轮加权(Σcached / Σinput);尚无样本时不展示
const cacheHitRate = computed(() => {
  const input = session.value.cacheHitInputTokens;
  if (input <= 0) return null;
  return session.value.cacheHitCachedTokens / input;
});

// --- 消息渲染:按「用户消息 / assistant 回合」归组 ---
// 一个回合 = 连续的 assistant 消息(一轮问答的多轮工具循环),各轮次的思考
// 与工具调用统一收进 ChatTurnProcess 折叠块;忙时流式过程/正文并入末尾回合,
// 最终正文开始输出后过程块随 active 翻转整体自动收起,回答之上只留一行摘要。

interface TurnView {
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
}

type TimelineView = { kind: "user"; key: string; content: string; editable: boolean } | TurnView;

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
  };
}

const timeline = computed<TimelineView[]>(() => {
  const s = session.value;
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
});

const retryClock = useNow({ interval: 250 });
const retrySeconds = computed(() => {
  const retry = session.value.retry;
  if (!retry) return 0;
  const elapsed = retryClock.value.getTime() - retry.scheduledAt;
  return Math.max(0, Math.ceil((retry.delayMs - elapsed) / 1000));
});
</script>

<template>
  <div class="pointer-events-none fixed right-4 bottom-4 z-50">
    <!-- 展开态:右下角固定面板(宽度/高度过渡驱动放大还原动画) -->
    <Transition name="chat-dock">
      <div
        v-if="open"
        class="chat-panel pointer-events-auto relative flex origin-bottom-right flex-col overflow-hidden rounded-xl border bg-background shadow-lg"
        :class="{ 'chat-panel-resizing': resizing }"
        :style="panelStyle"
      >
        <!-- 头部:标题 + 项目名 + 新会话 + 关闭 -->
        <div class="flex shrink-0 items-center justify-between gap-2 border-b px-3 py-2">
          <div class="flex min-w-0 items-center gap-2">
            <Bot class="h-4 w-4 shrink-0 text-muted-foreground" />
            <span class="shrink-0 text-sm font-semibold">{{ t("chat.title") }}</span>
            <span class="truncate text-xs text-muted-foreground">{{ project.name }}</span>
          </div>
          <div class="flex shrink-0 items-center gap-0.5">
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="t('chat.newSession')"
              @click="startNewSession"
            >
              <RotateCcw class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="expanded ? t('chat.restore') : t('chat.expand')"
              @click="toggleExpanded"
            >
              <Minimize2 v-if="expanded" class="h-3.5 w-3.5" />
              <Maximize2 v-else class="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-7 w-7"
              :title="t('chat.close')"
              @click="toggleOpen"
            >
              <X class="h-4 w-4" />
            </Button>
          </div>
        </div>

        <!-- 消息区 -->
        <Conversation class="min-h-0 flex-1">
          <ConversationContent class="gap-4 px-3">
            <ConversationEmptyState v-if="isEmpty">
              <div class="flex flex-col items-center gap-3 text-center">
                <Bot class="size-8 text-muted-foreground" />
                <div class="space-y-1">
                  <h3 class="text-sm font-medium">{{ t("chat.emptyTitle") }}</h3>
                  <p class="text-muted-foreground text-sm">{{ t("chat.emptyHint") }}</p>
                </div>
                <div class="flex flex-wrap justify-center gap-2">
                  <Button
                    v-for="suggestion in suggestions"
                    :key="suggestion"
                    variant="outline"
                    size="sm"
                    :disabled="!aiReady"
                    @click="sendText(suggestion)"
                  >
                    {{ suggestion }}
                  </Button>
                </div>
                <p v-if="!aiReady" class="text-muted-foreground text-xs">
                  {{ t("chat.notConfigured") }}
                  <Button
                    variant="link"
                    size="sm"
                    class="h-auto p-0 text-xs"
                    @click="router.push('/settings')"
                  >
                    {{ t("chat.goSettings") }}
                  </Button>
                </p>
              </div>
            </ConversationEmptyState>

            <template v-else>
              <template v-for="view in timeline" :key="view.key">
                <Message v-if="view.kind === 'user'" from="user" class="max-w-[90%]">
                  <!-- 编辑态:气泡换成内联 textarea,Enter 确认重发 / Esc 取消
                       (Esc 拦截冒泡,避免触发面板级 Esc 收起) -->
                  <MessageContent v-if="editingKey === view.key" class="w-full min-w-0">
                    <textarea
                      ref="editTextarea"
                      v-model="editText"
                      :rows="editRows"
                      class="w-full resize-none bg-transparent outline-none"
                      @keydown.enter.exact.prevent="confirmEdit"
                      @keydown.esc.stop="cancelEdit"
                    />
                    <div class="flex items-center justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="sm"
                        class="h-7 px-2 text-xs"
                        @click="cancelEdit"
                      >
                        {{ t("common.cancel") }}
                      </Button>
                      <Button
                        size="sm"
                        class="h-7 px-2 text-xs"
                        :disabled="!editText.trim() || !aiReady"
                        @click="confirmEdit"
                      >
                        {{ t("chat.send") }}
                      </Button>
                    </div>
                  </MessageContent>
                  <template v-else>
                    <!-- 操作钮固定占位(opacity 切换),悬停气泡时显示,不引起布局位移;
                         复制对所有提问可用,编辑仅最后一条 -->
                    <MessageActions
                      class="shrink-0 self-center opacity-0 transition-opacity group-hover:opacity-100"
                    >
                      <MessageAction
                        v-if="view.editable"
                        :tooltip="t('chat.editMessage')"
                        @click="startEdit(view)"
                      >
                        <Pencil class="size-3.5" />
                      </MessageAction>
                      <MessageAction :tooltip="t('chat.copy')" @click="copyText(view.content)">
                        <Copy class="size-3.5" />
                      </MessageAction>
                    </MessageActions>
                    <MessageContent>
                      <span class="whitespace-pre-wrap">{{ view.content }}</span>
                    </MessageContent>
                  </template>
                </Message>
                <!-- assistant 回合:思考与工具的统一折叠块 + 各段正文 -->
                <Message v-else from="assistant" class="max-w-[90%]">
                  <div class="flex min-w-0 flex-col">
                    <MessageContent>
                      <ChatTurnProcess
                        v-if="view.groups.length > 0"
                        :groups="view.groups"
                        :active="view.active"
                        @respond="onToolPermissionRespond"
                      />
                      <MessageResponse
                        v-for="(content, ci) in view.contents"
                        :key="ci"
                        :content="content"
                      />
                      <template v-if="view.live">
                        <MessageResponse
                          v-if="view.streamingText.trim()"
                          :content="view.streamingText"
                          mode="streaming"
                        />
                        <div
                          v-else-if="view.retry"
                          class="flex items-center gap-2 text-muted-foreground text-xs"
                          :title="view.retry.message"
                        >
                          <Loader />
                          <span>
                            {{
                              t("chat.retryScheduled", {
                                attempt: view.retry.attempt,
                                max: view.retry.maxAttempts,
                                seconds: retrySeconds,
                              })
                            }}
                          </span>
                        </div>
                        <!-- 过程折叠块头部已带「正在思考与执行…」旋转图标,仅在尚未
                           产出任何过程(无思考、无工具)时才需要独立的 Loader -->
                        <Loader
                          v-else-if="view.groups.length === 0"
                          class="text-muted-foreground"
                        />
                      </template>
                    </MessageContent>
                    <!-- 回合复制:悬停显现;流式中(正文未定型)不显示 -->
                    <MessageActions
                      v-if="!view.live && view.contents.length > 0"
                      class="opacity-0 transition-opacity group-hover:opacity-100"
                    >
                      <MessageAction :tooltip="t('chat.copy')" @click="copyTurn(view)">
                        <Copy class="size-3.5" />
                      </MessageAction>
                    </MessageActions>
                  </div>
                </Message>
              </template>
            </template>
          </ConversationContent>
          <ConversationScrollButton />
        </Conversation>

        <!-- 错误条(最终失败 / 流失败;忙时发送被拒的提示也走这里) -->
        <div
          v-if="session.error"
          class="flex shrink-0 items-start gap-2 border-t bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          <TriangleAlert class="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span class="min-w-0 flex-1 whitespace-pre-wrap">{{ session.error }}</span>
        </div>

        <!-- 输入区 -->
        <div class="shrink-0 p-3">
          <PromptInput class="rounded-lg" @submit="onSubmit">
            <PromptInputBody>
              <PromptInputTextarea
                class="min-h-9"
                :placeholder="t('chat.composer.placeholder')"
                :disabled="!aiReady"
              />
            </PromptInputBody>
            <PromptInputFooter class="px-2 pb-2">
              <PromptInputTools ref="tools" class="min-w-0 flex-1 flex-wrap">
                <!-- body-lock 必须与 disable-outside-pointer-events=false 成对出现:
                     reka Select 的 bodyLock 默认 true 会给 body 写 pointer-events:none,
                     内容不自恢复 auto 时选项无法点击(见 ModelSelector.vue 注释) -->
                <Select
                  :model-value="aiConfig.chatPermission"
                  :disabled="session.busy"
                  @update:model-value="onPermissionChange"
                >
                  <SelectTrigger
                    size="sm"
                    class="text-muted-foreground h-7 gap-1 px-2 text-xs"
                    :title="permissionTitle"
                  >
                    <Eye v-if="aiConfig.chatPermission === 'ask'" class="size-3.5 shrink-0" />
                    <Wrench v-else class="size-3.5 shrink-0" />
                    <span>
                      {{
                        aiConfig.chatPermission === "all"
                          ? t("chat.permission.allShort")
                          : t("chat.permission.askShort")
                      }}
                    </span>
                  </SelectTrigger>
                  <SelectContent :disable-outside-pointer-events="false" :body-lock="false">
                    <SelectItem value="ask" class="text-xs" :title="t('chat.permission.ask')">
                      {{ t("chat.permission.askShort") }}
                    </SelectItem>
                    <SelectItem value="all" class="text-xs" :title="t('chat.permission.all')">
                      {{ t("chat.permission.allShort") }}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <ModelSelector
                  :model-value="aiConfig.chatModelValue"
                  :groups="modelGroups"
                  :disabled="!aiReady || session.busy"
                  :placeholder="t('chat.modelPlaceholder')"
                  @update:model-value="onModelChange"
                />
                <Select
                  :model-value="aiConfig.chatThinking"
                  :disabled="!aiReady || thinkingDisabled || session.busy"
                  @update:model-value="onThinkingChange"
                >
                  <SelectTrigger
                    size="sm"
                    class="text-muted-foreground h-7 gap-1 px-2 text-xs"
                    :title="thinkingTitle"
                  >
                    <Brain class="size-3.5 shrink-0" />
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent :disable-outside-pointer-events="false" :body-lock="false">
                    <SelectItem
                      v-for="level in CHAT_THINKING_LEVELS"
                      :key="level"
                      :value="level"
                      class="text-xs"
                    >
                      {{ t(`chat.thinkingLevels.${level}`) }}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <p v-if="!aiReady" class="text-muted-foreground w-full text-xs">
                  {{ t("chat.notConfigured") }}
                  <Button
                    variant="link"
                    size="sm"
                    class="h-auto p-0 text-xs"
                    @click="router.push('/settings')"
                  >
                    {{ t("chat.goSettings") }}
                  </Button>
                </p>
              </PromptInputTools>
              <!-- 上下文占用:首个回答产出用量数据前不显示 -->
              <Context
                v-if="session.contextTokens != null"
                :used-tokens="session.contextTokens"
                :max-tokens="contextWindow"
                :usage="contextUsage"
                :cost="contextCost"
                :breakdown="session.contextBreakdown"
                :cache-hit-rate="cacheHitRate"
              >
                <ContextTrigger />
                <ContextContent side="top" align="end">
                  <ContextContentHeader />
                  <ContextContentBody class="space-y-2">
                    <ContextBreakdownUsage />
                    <ContextCacheHitRate />
                  </ContextContentBody>
                  <ContextContentFooter />
                </ContextContent>
              </Context>
              <!-- 发送/停止共用槽位:忙时提交钮隐藏(disabled 保留在 DOM 供 Enter 守卫),同位显示方形停止钮 -->
              <div class="relative size-8 shrink-0">
                <PromptInputSubmit
                  v-show="!session.busy"
                  :status="submitStatus"
                  :disabled="!aiReady || session.busy"
                />
                <PromptInputButton
                  v-if="session.busy"
                  variant="destructive"
                  class="absolute inset-0"
                  :title="t('chat.stop')"
                  @click="abort"
                >
                  <Square class="size-3.5 fill-current" />
                </PromptInputButton>
              </div>
            </PromptInputFooter>
          </PromptInput>
        </div>

        <!-- 边缘拖拽手柄:左缘加宽、上缘加高、左上角同时调整(面板锚定右下角) -->
        <div
          class="absolute top-0 bottom-0 left-0 w-1.5 cursor-ew-resize touch-none"
          @pointerdown="startResize('x', $event)"
        />
        <div
          class="absolute top-0 right-0 left-0 h-1.5 cursor-ns-resize touch-none"
          @pointerdown="startResize('y', $event)"
        />
        <div
          class="absolute top-0 left-0 size-3 cursor-nwse-resize touch-none"
          @pointerdown="startResize('both', $event)"
        />
      </div>
    </Transition>

    <!-- 收起态:圆形入口,锚定容器右下角(回答中叠加脉冲状态点)。
         绝对定位脱离面板布局,关闭动画期间即落在最终位置,不再先出现在面板上方 -->
    <Transition name="chat-entry">
      <button
        v-if="!open"
        type="button"
        class="pointer-events-auto absolute right-0 bottom-0 flex h-12 w-12 items-center justify-center rounded-full border bg-card shadow-lg transition-shadow hover:shadow-xl"
        :title="t('chat.entry')"
        @click="toggleOpen"
      >
        <MessageCircleMore class="h-5 w-5 text-foreground" />
        <span
          v-if="session.busy"
          class="absolute top-0 right-0 size-2.5 animate-pulse rounded-full bg-green-500 ring-2 ring-background"
        />
      </button>
    </Transition>
  </div>
</template>

<style scoped>
/* 放大/还原:尺寸类切换时平滑过渡(开合期间由下方 enter/leave 规则接管,避免冲突) */
.chat-panel {
  transition:
    width 0.25s ease,
    height 0.25s ease,
    max-width 0.25s ease,
    max-height 0.25s ease;
}

/* 拖拽改尺寸时禁用过渡,面板紧随指针 */
.chat-panel-resizing {
  transition: none;
}

/* 打开/关闭:面板朝右下角缩放淡出,与入口按钮位置呼应 */
.chat-dock-enter-active,
.chat-dock-leave-active {
  transition:
    opacity 0.2s ease,
    transform 0.2s ease;
}

.chat-dock-enter-from,
.chat-dock-leave-to {
  opacity: 0;
  transform: translateY(12px) scale(0.95);
}

/* 入口按钮:打开时快速淡出;关闭时延迟淡入,等面板收拢后再出现 */
.chat-entry-enter-active {
  transition:
    opacity 0.18s ease 0.12s,
    transform 0.18s ease 0.12s;
}

.chat-entry-leave-active {
  transition:
    opacity 0.12s ease,
    transform 0.12s ease;
}

.chat-entry-enter-from,
.chat-entry-leave-to {
  opacity: 0;
  transform: scale(0.75);
}
</style>
