<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { toast } from "vue-sonner";
import {
  Bot,
  Brain,
  Eye,
  Maximize2,
  MessageCircleMore,
  Minimize2,
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
import { Message, MessageContent, MessageResponse } from "@/components/ai-elements/message";
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
import { CHAT_THINKING_LEVELS, type ChatThinkingLevel } from "@/lib/ai-config";
import type { ChatProcessGroup, ChatToolRun } from "@/lib/chat";
import { useAiConfigStore } from "@/stores/ai-config";
import { useChatStore, type ChatRetryState } from "@/stores/chat";
import type { Project } from "@/types";

const props = defineProps<{ project: Project }>();

const { t } = useI18n();
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

// --- 放大/还原:放大时面板扩展为大尺寸(接近全高、加宽) ---
const expanded = ref(false);

function toggleExpanded() {
  expanded.value = !expanded.value;
}

// 高度让出自绘标题栏(TitleBar.vue h-9 = 2.25rem,z-60 盖在浮层 z-50 之上,
// 约定同 lib/popper.ts):底部边距 1rem + 标题栏下间隙 1rem + 标题栏 2.25rem
const panelSizeClass = computed(() =>
  expanded.value
    ? "h-[calc(100vh-2rem-2.25rem)] w-[720px] max-w-[calc(100vw-2rem)]"
    : "h-[640px] max-h-[calc(100vh-2rem-2.25rem)] w-[420px] max-w-[calc(100vw-2rem)]",
);

// --- 发送 / 停止 / 新会话 ---
function onSubmit(message: PromptInputMessage) {
  sendText(message.text);
}

function sendText(text: string) {
  const trimmed = text.trim();
  if (!trimmed || !aiReady.value || session.value.busy) return;
  void chat.send(props.project.path, props.project, trimmed);
}

function abort() {
  chat.abort(props.project.path);
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
  aiConfig.chatPermission === "all" ? t("chat.permission.all") : t("chat.permission.readOnly"),
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

function togglePermission() {
  const next = aiConfig.chatPermission === "all" ? "readOnly" : "all";
  void applyPref(() => aiConfig.setChatPermission(next));
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

type TimelineView = { kind: "user"; key: string; content: string } | TurnView;

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
  for (const message of s.messages) {
    if (message.role === "user") {
      turn = null;
      views.push({ kind: "user", key: message.id, content: message.content });
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
        class="chat-panel pointer-events-auto flex origin-bottom-right flex-col overflow-hidden rounded-xl border bg-background shadow-lg"
        :class="panelSizeClass"
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
                  <MessageContent>
                    <span class="whitespace-pre-wrap">{{ view.content }}</span>
                  </MessageContent>
                </Message>
                <!-- assistant 回合:思考与工具的统一折叠块 + 各段正文 -->
                <Message v-else from="assistant" class="max-w-[90%]">
                  <MessageContent>
                    <ChatTurnProcess
                      v-if="view.groups.length > 0"
                      :groups="view.groups"
                      :active="view.active"
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
                      <Loader v-else class="text-muted-foreground" />
                    </template>
                  </MessageContent>
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
              <PromptInputTools class="min-w-0 flex-1 flex-wrap">
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
                  <SelectContent>
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
                <PromptInputButton
                  class="text-muted-foreground hover:text-foreground size-7"
                  :title="permissionTitle"
                  :disabled="session.busy"
                  @click="togglePermission"
                >
                  <Eye v-if="aiConfig.chatPermission === 'readOnly'" class="size-3.5" />
                  <Wrench v-else class="size-3.5" />
                </PromptInputButton>
                <Context
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
