<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { RouterLink } from "vue-router";
import { FoldVertical, Square, TriangleAlert } from "@lucide/vue";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import {
  PromptInput,
  PromptInputBody,
  PromptInputButton,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
} from "@/components/ai-elements/prompt-input";
import { formatTokenCount } from "@/lib/chat";
import { useAiConfigStore } from "@/stores/ai-config";
import { useChatStore } from "@/stores/chat";
import type { Project } from "@/types";
import { useChatDockLayout } from "@/composables/chat-dock/use-chat-dock-layout";
import { useChatTimeline } from "@/composables/chat-dock/use-chat-timeline";
import { useChatMessageActions } from "@/composables/chat-dock/use-chat-message-actions";
import ChatDockHeader from "@/components/chat/dock/ChatDockHeader.vue";
import ChatDockEmptyState from "@/components/chat/dock/ChatDockEmptyState.vue";
import ChatUserMessage from "@/components/chat/dock/ChatUserMessage.vue";
import ChatAssistantTurn from "@/components/chat/dock/ChatAssistantTurn.vue";
import ChatDockPrefs from "@/components/chat/dock/ChatDockPrefs.vue";
import ChatContextMeter from "@/components/chat/dock/ChatContextMeter.vue";

const props = defineProps<{ project: Project }>();
const open = defineModel<boolean>("open", { default: false });

const { t } = useI18n();
const chat = useChatStore();
const aiConfig = useAiConfigStore();

// --- AI 配置就绪(多厂商配置 ai-config.json;chat 模型有效且厂商 baseUrl/apiKey 齐) ---
onMounted(() => {
  void aiConfig.ensureLoaded();
});
const aiReady = computed(() => aiConfig.chatReady);

// --- 会话状态(按项目路径隔离,离开页面不清空) ---
const session = computed(() => chat.ensureSession(props.project.path));

const isEmpty = computed(() => session.value.messages.length === 0 && !session.value.busy);

// ── 面板布局 / 时间线视图 / 消息动作(实现见 composables/chat-dock/*) ──
const { expanded, toggleExpanded, panelStyle, resizing, startResize } = useChatDockLayout(open);
const { timeline, retrySeconds } = useChatTimeline(session);
const {
  onSubmit,
  sendText,
  abort,
  startNewSession,
  editingKey,
  editText,
  startEdit,
  cancelEdit,
  confirmEdit,
  copyText,
  copyTurn,
  submitStatus,
} = useChatMessageActions({ project: () => props.project, session, aiReady });
</script>

<template>
  <aside
    v-show="open"
    ref="chatPanel"
    class="chat-panel relative flex h-full min-h-0 shrink-0 flex-col overflow-hidden border-l bg-background"
    :class="{ 'chat-panel-resizing': resizing }"
    :style="panelStyle"
    :aria-label="t('chat.title')"
  >
    <ChatDockHeader
      :project-name="project.name"
      :expanded="expanded"
      @new-session="startNewSession"
      @toggle-expanded="toggleExpanded"
      @close="open = false"
    />

    <!-- 消息区 -->
    <Conversation class="min-h-0 flex-1">
      <ConversationContent class="gap-4 px-3">
        <ConversationEmptyState v-if="isEmpty">
          <ChatDockEmptyState :ai-ready="aiReady" @send="sendText" />
        </ConversationEmptyState>

        <template v-else>
          <template v-for="view in timeline" :key="view.key">
            <ChatUserMessage
              v-if="view.kind === 'user'"
              v-model:edit-text="editText"
              :content="view.content"
              :editable="view.editable"
              :editing="editingKey === view.key"
              :ai-ready="aiReady"
              @start-edit="startEdit(view)"
              @confirm="confirmEdit"
              @cancel="cancelEdit"
              @copy="copyText(view.content)"
            />
            <!-- 自动压缩时间线标记(分隔条样式,居中) -->
            <div
              v-else-if="view.kind === 'compaction'"
              class="flex items-center gap-2 px-2 py-1 text-muted-foreground text-xs"
              :title="t('chat.compactionDoneTitle')"
            >
              <div class="h-px flex-1 bg-border" />
              <FoldVertical class="size-3.5 shrink-0" />
              <span>
                {{
                  t("chat.compactionDone", {
                    before: formatTokenCount(view.tokensBefore),
                    after: formatTokenCount(view.tokensAfter),
                  })
                }}
              </span>
              <div class="h-px flex-1 bg-border" />
            </div>
            <ChatAssistantTurn
              v-else
              :turn="view"
              :retry-seconds="retrySeconds"
              :project-path="project.path"
              @copy="copyTurn(view)"
            />
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
          <PromptInputTools class="min-w-0 flex-1 flex-nowrap">
            <ChatDockPrefs :busy="session.busy" :ai-ready="aiReady" />
          </PromptInputTools>
          <div class="flex shrink-0 items-center gap-1">
            <ChatContextMeter :session="session" />
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
          </div>
        </PromptInputFooter>
      </PromptInput>
      <p v-if="!aiReady" class="mt-2 text-xs text-muted-foreground">
        {{ t("chat.notConfigured") }}
        <RouterLink to="/settings" class="text-primary hover:underline">
          {{ t("chat.goSettings") }}
        </RouterLink>
      </p>
    </div>

    <!-- 右侧栏仅调整宽度,高度始终跟随项目工作区。 -->
    <div
      class="absolute inset-y-0 left-0 w-1.5 cursor-ew-resize touch-none hover:bg-primary/20"
      @pointerdown="startResize"
    />
  </aside>
</template>

<style scoped>
.chat-panel {
  transition: width 0.2s ease;
}

.chat-panel-resizing {
  transition: none;
}

@media (prefers-reduced-motion: reduce) {
  .chat-panel {
    transition: none;
  }
}
</style>
