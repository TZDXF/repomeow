<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { Copy } from "@lucide/vue";
import { Loader } from "@/components/ai-elements/loader";
import {
  Message,
  MessageAction,
  MessageActions,
  MessageContent,
  MessageResponse,
} from "@/components/ai-elements/message";
import ChatTurnProcess from "@/components/chat/ChatTurnProcess.vue";
import { useChatStore } from "@/stores/chat";
import type { TurnView } from "@/composables/chat-dock/use-chat-timeline";

// ── assistant 回合:思考与工具的统一折叠块 + 各段正文 + 流式/重试/压缩状态 ──
const props = defineProps<{
  turn: TurnView;
  /** 重试倒计时剩余秒数(父层 250ms tick 驱动) */
  retrySeconds: number;
  projectPath: string;
}>();
const emit = defineEmits<{
  copy: [];
}>();
const { t } = useI18n();
const chat = useChatStore();

/** ChatToolCard 冒泡的工具权限请求:经 store 回应后端;失败 toast 提示可重试 */
function onToolPermissionRespond(payload: { id: string; allow: boolean }) {
  void chat.respondToolPermission(props.projectPath, payload.id, payload.allow).then((ok) => {
    if (!ok) toast.error(t("chat.toolPermission.respondFailed"));
  });
}
</script>

<template>
  <Message from="assistant" class="max-w-[90%]">
    <div class="flex min-w-0 flex-col">
      <MessageContent>
        <ChatTurnProcess
          v-if="turn.groups.length > 0"
          :groups="turn.groups"
          :active="turn.active"
          @respond="onToolPermissionRespond"
        />
        <MessageResponse v-for="(content, ci) in turn.contents" :key="ci" :content="content" />
        <template v-if="turn.live">
          <MessageResponse
            v-if="turn.streamingText.trim()"
            :content="turn.streamingText"
            mode="streaming"
          />
          <div
            v-else-if="turn.compacting"
            class="flex items-center gap-2 text-muted-foreground text-xs"
          >
            <Loader />
            <span>{{ t("chat.compacting") }}</span>
          </div>
          <div
            v-else-if="turn.retry"
            class="flex items-center gap-2 text-muted-foreground text-xs"
            :title="turn.retry.message"
          >
            <Loader />
            <span>
              {{
                t("chat.retryScheduled", {
                  attempt: turn.retry.attempt,
                  max: turn.retry.maxAttempts,
                  seconds: retrySeconds,
                })
              }}
            </span>
          </div>
          <!-- 过程折叠块头部已带「正在思考与执行…」旋转图标,仅在尚未
             产出任何过程(无思考、无工具)时才需要独立的 Loader -->
          <Loader v-else-if="turn.groups.length === 0" class="text-muted-foreground" />
        </template>
        <!-- 回合复制仅在回答完成后显示，位于回答正文左下方 -->
        <MessageActions v-if="!turn.live && turn.contents.length > 0" class="self-start">
          <MessageAction :tooltip="t('chat.copy')" @click="emit('copy')">
            <Copy class="size-3.5" />
          </MessageAction>
        </MessageActions>
      </MessageContent>
    </div>
  </Message>
</template>
