<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import { Bot, MessageCircleMore, RotateCcw, Square, TriangleAlert, X } from "@lucide/vue";
import { useEventListener } from "@vueuse/core";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Loader } from "@/components/ai-elements/loader";
import { Message, MessageContent, MessageResponse } from "@/components/ai-elements/message";
import {
  PromptInput,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  type PromptInputMessage,
} from "@/components/ai-elements/prompt-input";
import { Button } from "@/components/ui/button";
import ChatToolCard from "@/components/chat/ChatToolCard.vue";
import type { ChatMessage } from "@/lib/chat";
import { useChatStore } from "@/stores/chat";
import { useSettingsStore } from "@/stores/settings";
import type { Project } from "@/types";

const props = defineProps<{ project: Project }>();

const { t } = useI18n();
const router = useRouter();
const chat = useChatStore();
const settings = useSettingsStore();

// --- AI 配置校验(baseUrl/apiKey/model 任一为空都视为未配置) ---
const aiReady = computed(() =>
  Boolean(settings.aiBaseUrl && settings.aiApiKey && settings.aiModel),
);

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

function startNewSession() {
  // 忙时直接清:store 内部先中止在途请求,等待落地后重置前后端会话
  void chat.newSession(props.project.path);
}

// --- 用量摘要(done 后显示在输入区左侧) ---
const usageText = computed(() => {
  const usage = session.value.lastUsage;
  if (!usage || session.value.busy) return "";
  return t("chat.usageHint", { input: usage.inputTokens, output: usage.outputTokens });
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
        class="pointer-events-auto flex h-[640px] max-h-[calc(100vh-2rem)] w-[420px] max-w-[calc(100vw-2rem)] flex-col overflow-hidden rounded-xl border bg-background shadow-lg"
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
            <PromptInputTextarea
              class="min-h-9"
              :placeholder="t('chat.composer.placeholder')"
              :disabled="!aiReady"
            />
            <PromptInputFooter class="px-2 pb-2">
              <span class="min-w-0 truncate text-muted-foreground text-xs">
                <template v-if="!aiReady">{{ t("chat.notConfigured") }}</template>
                <template v-else-if="session.busy">{{ t("chat.busyHint") }}</template>
                <template v-else-if="usageText">{{ usageText }}</template>
              </span>
              <div class="flex shrink-0 items-center gap-1.5">
                <Button
                  v-if="session.busy"
                  size="sm"
                  variant="destructive"
                  class="h-8 gap-1 px-2"
                  @click="abort"
                >
                  <Square class="h-3 w-3 fill-current" />
                  {{ t("chat.stop") }}
                </Button>
                <PromptInputSubmit :disabled="!aiReady || session.busy" />
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
