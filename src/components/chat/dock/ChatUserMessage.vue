<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Copy, Pencil } from "@lucide/vue";
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent,
} from "@/components/ai-elements/message";
import { Button } from "@/components/ui/button";

// ── 用户提问气泡:悬停操作钮(复制/编辑),编辑态换成内联 textarea,
// Enter 确认重发 / Esc 取消(Esc 拦截冒泡,避免触发面板级 Esc 收起) ---
const props = defineProps<{
  content: string;
  /** 仅最后一条提问可编辑(父层在忙时也会关掉) */
  editable: boolean;
  /** 当前处于编辑态(父层 editingKey 命中本条) */
  editing: boolean;
  aiReady: boolean;
}>();
const editText = defineModel<string>("editText", { required: true });
const emit = defineEmits<{
  startEdit: [];
  confirm: [];
  cancel: [];
  copy: [];
}>();
const { t } = useI18n();

const editTextareaRef = ref<HTMLTextAreaElement | null>(null);
const editRows = computed(() => Math.min(10, Math.max(2, editText.value.split("\n").length)));

// 进入编辑态后聚焦并把光标移到文末
watch(
  () => props.editing,
  (on) => {
    if (!on) return;
    void nextTick(() => {
      const el = editTextareaRef.value;
      if (el) {
        el.focus();
        el.setSelectionRange(el.value.length, el.value.length);
      }
    });
  },
);
</script>

<template>
  <Message from="user" class="max-w-[90%] flex-col items-end">
    <MessageContent v-if="editing" class="w-full min-w-0">
      <textarea
        ref="editTextareaRef"
        v-model="editText"
        :rows="editRows"
        class="w-full resize-none bg-transparent outline-none"
        @keydown.enter.exact.prevent="emit('confirm')"
        @keydown.esc.stop="emit('cancel')"
      />
      <div class="flex items-center justify-end gap-1">
        <Button variant="ghost" size="sm" class="h-7 px-2 text-xs" @click="emit('cancel')">
          {{ t("common.cancel") }}
        </Button>
        <Button
          size="sm"
          class="h-7 px-2 text-xs"
          :disabled="!editText.trim() || !aiReady"
          @click="emit('confirm')"
        >
          {{ t("chat.send") }}
        </Button>
      </div>
    </MessageContent>
    <template v-else>
      <MessageContent>
        <span class="whitespace-pre-wrap">{{ content }}</span>
      </MessageContent>
      <!-- 操作钮位于气泡右下方,固定占位(opacity 切换),悬停气泡时显示,不引起布局位移;
           复制对所有提问可用,编辑仅最后一条 -->
      <MessageActions
        class="shrink-0 self-end opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
      >
        <MessageAction v-if="editable" :tooltip="t('chat.editMessage')" @click="emit('startEdit')">
          <Pencil class="size-3.5" />
        </MessageAction>
        <MessageAction :tooltip="t('chat.copy')" @click="emit('copy')">
          <Copy class="size-3.5" />
        </MessageAction>
      </MessageActions>
    </template>
  </Message>
</template>
