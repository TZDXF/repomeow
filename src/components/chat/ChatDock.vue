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
import { Context } from "@/components/ai-elements/context";
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
import ChatToolCard from "@/components/chat/ChatToolCard.vue";
import { CHAT_THINKING_LEVELS, type ChatThinkingLevel } from "@/lib/ai-config";
import type { ChatMessage } from "@/lib/chat";
import { useAiConfigStore } from "@/stores/ai-config";
import { useChatStore } from "@/stores/chat";
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

const lastUsageDetail = computed(() => {
  const usage = session.value.lastUsage;
  if (!usage) return null;
  return {
    inputTokens: usage.inputTokens,
    outputTokens: usage.outputTokens,
    cachedTokens: usage.cachedTokens,
  };
});

// --- 消息渲染辅助 ---
function toolRunsOf(message: ChatMessage) {
  return message.toolRunIds.map((id) => session.value.toolRuns[id]).filter((run) => run != null);
}

const pendingToolRuns = computed(() =>
  session.value.pendingToolRunIds
    .map((id) => session.value.toolRuns[id])
    .filter((run) => run != null),
);

const retryClock = useNow({ interval: 250 });
const retrySeconds = computed(() => {
  const retry = session.value.retry;
  if (!retry) return 0;
  const elapsed = retryClock.value.getTime() - retry.scheduledAt;
  return Math.max(0, Math.ceil((retry.delayMs - elapsed) / 1000));
});
</script>

<template>
  <div class="pointer-events-none fixed right-4 bottom-4 z-50 flex flex-col items-end">
    <!-- 收起态:圆形入口;回答中叠加脉冲状态点 -->
    <button
      v-if="!open"
      type="button"
      class="pointer-events-auto relative flex h-12 w-12 items-center justify-center rounded-full border bg-card shadow-lg transition-shadow hover:shadow-xl"
      :title="t('chat.entry')"
      @click="toggleOpen"
    >
      <MessageCircleMore class="h-5 w-5 text-foreground" />
      <span
        v-if="session.busy"
        class="absolute top-0 right-0 size-2.5 animate-pulse rounded-full bg-green-500 ring-2 ring-background"
      />
    </button>

    <!-- 展开态:右下角固定面板 -->
    <Transition name="chat-dock">
      <div
        v-if="open"
        class="pointer-events-auto flex flex-col overflow-hidden rounded-xl border bg-background shadow-lg"
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
              <Message
                v-for="message in session.messages"
                :key="message.id"
                :from="message.role"
                class="max-w-[90%]"
              >
                <div
                  v-if="message.role === 'assistant'"
                  class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full border bg-muted"
                >
                  <Bot class="size-4 text-muted-foreground" />
                </div>
                <MessageContent>
                  <ChatToolCard
                    v-for="(run, index) in toolRunsOf(message)"
                    :key="`${message.id}-${index}`"
                    :run="run"
                  />
                  <MessageResponse
                    v-if="message.role === 'assistant' && message.content"
                    :content="message.content"
                  />
                  <span v-else class="whitespace-pre-wrap">{{ message.content }}</span>
                </MessageContent>
              </Message>

              <!-- 流式中的回复:pending 工具卡片 + 累积文本 / Loader -->
              <Message v-if="session.busy" from="assistant" class="max-w-[90%]">
                <div
                  class="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full border bg-muted"
                >
                  <Bot class="size-4 text-muted-foreground" />
                </div>
                <MessageContent>
                  <ChatToolCard
                    v-for="(run, index) in pendingToolRuns"
                    :key="`pending-${index}`"
                    :run="run"
                  />
                  <MessageResponse v-if="session.streamingText" :content="session.streamingText" />
                  <div
                    v-else-if="session.retry"
                    class="flex items-center gap-2 text-xs text-muted-foreground"
                    :title="session.retry.message"
                  >
                    <Loader />
                    <span>
                      {{
                        t("chat.retryScheduled", {
                          attempt: session.retry.attempt,
                          max: session.retry.maxAttempts,
                          seconds: retrySeconds,
                        })
                      }}
                    </span>
                  </div>
                  <Loader v-else class="text-muted-foreground" />
                </MessageContent>
              </Message>
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
                  :context-window="contextWindow"
                  :last-usage="lastUsageDetail"
                />
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
  </div>
</template>

<style scoped>
.chat-dock-enter-active,
.chat-dock-leave-active {
  transition:
    opacity 0.15s ease,
    transform 0.15s ease;
}

.chat-dock-enter-from,
.chat-dock-leave-to {
  opacity: 0;
  transform: translateY(8px) scale(0.98);
}
</style>
