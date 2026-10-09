<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { FoldVertical, MessageCircleMore, Square, TriangleAlert } from "@lucide/vue";
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

const { t, locale } = useI18n();
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
const { open, toggleOpen, expanded, toggleExpanded, panelStyle, resizing, startResize } =
  useChatDockLayout([
    () => aiConfig.chatModelValue,
    () => aiConfig.chatPermission,
    () => locale.value,
  ]);
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
  <div class="pointer-events-none fixed right-4 bottom-4 z-50">
    <!-- 展开态:右下角固定面板(宽度/高度过渡驱动放大还原动画) -->
    <Transition name="chat-dock">
      <div
        v-if="open"
        class="chat-panel pointer-events-auto relative flex origin-bottom-right flex-col overflow-hidden rounded-xl border bg-background shadow-lg"
        :class="{ 'chat-panel-resizing': resizing }"
        :style="panelStyle"
      >
        <ChatDockHeader
          :project-name="project.name"
          :expanded="expanded"
          @new-session="startNewSession"
          @toggle-expanded="toggleExpanded"
          @close="toggleOpen"
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
              <PromptInputTools ref="tools" class="min-w-0 flex-1 flex-wrap">
                <ChatDockPrefs :busy="session.busy" :ai-ready="aiReady" />
              </PromptInputTools>
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
        class="chat-fab pointer-events-auto absolute right-0 bottom-0 flex h-12 w-12 items-center justify-center rounded-full border bg-card shadow-lg transition-shadow hover:shadow-xl"
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
