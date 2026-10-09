import { computed, ref, toValue, type ComputedRef, type MaybeRefOrGetter } from "vue";
import type { ChatStatus, PromptInputMessage } from "@/components/ai-elements/prompt-input";
import { copyToClipboard } from "@/lib/utils";
import { friendlyChatError, useChatStore, type ChatSessionState } from "@/stores/chat";
import type { Project } from "@/types";
import type { TurnView } from "./use-chat-timeline";

export interface ChatMessageActionsOptions {
  project: MaybeRefOrGetter<Project>;
  session: ComputedRef<ChatSessionState>;
  aiReady: ComputedRef<boolean>;
}

/**
 * 会话消息动作:发送/停止/新会话、上一条提问的内联编辑重发、复制。
 * 编辑焦点管理在 ChatUserMessage 内(editing 变 true 后自行聚焦),
 * 这里只维护 editingKey/editText 状态。
 */
export function useChatMessageActions(options: ChatMessageActionsOptions) {
  const chat = useChatStore();

  // --- 发送 / 停止 / 新会话 ---
  function onSubmit(message: PromptInputMessage) {
    return sendText(message.text);
  }

  function sendText(text: string) {
    const trimmed = text.trim();
    if (!trimmed) return false;
    if (!options.aiReady.value) {
      options.session.value.error = friendlyChatError("ai_not_configured", "");
      return false;
    }
    if (options.session.value.busy) {
      options.session.value.error = friendlyChatError("ai_request_failed", "chat_busy");
      return false;
    }
    cancelEdit();
    // props 会在切换项目/工作树时被替换,发送时才解析,不能保存 setup 时的快照。
    const project = toValue(options.project);
    return chat.send(project.path, project, trimmed);
  }

  function abort() {
    chat.abort(toValue(options.project).path);
  }

  function startNewSession() {
    // 忙时直接清:store 内部先中止在途请求,等待落地后重置前后端会话
    cancelEdit();
    void chat.newSession(toValue(options.project).path);
  }

  // --- 编辑上一条提问:最后一条用户消息悬停出现编辑钮,气泡内联编辑,
  // Enter 确认(截掉该提问的回答回合后以新文本重发),Esc 取消 ---
  const editingKey = ref<string | null>(null);
  const editText = ref("");

  function startEdit(view: { key: string; content: string }) {
    editingKey.value = view.key;
    editText.value = view.content;
  }

  function cancelEdit() {
    editingKey.value = null;
    editText.value = "";
  }

  function confirmEdit() {
    if (editingKey.value === null) return;
    const text = editText.value.trim();
    if (!text || !options.aiReady.value || options.session.value.busy) return;
    cancelEdit();
    const project = toValue(options.project);
    void chat.editLastUserMessage(project.path, project, text);
  }

  // --- 复制:提问复制原文;回答复制该回合的全部正文段(流式中不显示,
  // 避免复制到残缺文本)。成功/失败提示由 copyToClipboard 统一 toast ---
  function copyText(text: string) {
    void copyToClipboard(text);
  }

  function copyTurn(view: TurnView) {
    copyText(view.contents.join("\n\n"));
  }

  // 发送/停止共用一个位置(参照 ai-elements chatbot 示例):状态驱动提交按钮图标,
  // 等待首个 token 转圈、流式中显示方块;忙时提交钮隐藏但保留在 DOM 且 disabled,
  // 使输入框 Enter 守卫(button[type=submit] disabled 则不提交)继续生效、不误清已输入文本
  const submitStatus = computed<ChatStatus>(() => {
    if (options.session.value.busy)
      return options.session.value.streamingText ? "streaming" : "submitted";
    return options.session.value.error ? "error" : "ready";
  });

  return {
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
  };
}
