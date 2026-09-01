<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { BrainIcon, ChevronDownIcon, Loader2Icon } from "@lucide/vue";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { MessageResponse } from "@/components/ai-elements/message";
import { Shimmer } from "@/components/ai-elements/shimmer";
import type { ChatProcessGroup } from "@/lib/chat";
import ChatToolCard from "./ChatToolCard.vue";

/**
 * 一轮回答的「思考与执行过程」统一折叠块:各轮次的思考原文与工具卡片收在
 * 同一个 Collapsible 里,最终正文开始输出后整体收起,回答之上只留一行摘要。
 * active(过程仍在产出)时保持展开跟随流式,转 false 后延迟自动收起一次
 * (用户手动展开后不再干预);历史消息默认收起。
 */
const props = withDefaults(
  defineProps<{
    groups: ChatProcessGroup[];
    /** 过程仍在产出(本轮正文尚未开始):展开跟随流式;转 false 后自动收起 */
    active?: boolean;
    defaultOpen?: boolean;
  }>(),
  { active: false, defaultOpen: false },
);

const { t } = useI18n();

const open = ref(props.defaultOpen);
const hasAutoClosed = ref(false);

const AUTO_CLOSE_DELAY = 800;
let closeTimer: ReturnType<typeof setTimeout> | undefined;

watch(
  () => props.active,
  (isActive) => {
    if (closeTimer !== undefined) {
      clearTimeout(closeTimer);
      closeTimer = undefined;
    }
    if (isActive) {
      open.value = true;
      hasAutoClosed.value = false;
    } else if (open.value && !hasAutoClosed.value) {
      closeTimer = setTimeout(() => {
        closeTimer = undefined;
        open.value = false;
      }, AUTO_CLOSE_DELAY);
    }
  },
  { immediate: true },
);

onUnmounted(() => {
  if (closeTimer !== undefined) clearTimeout(closeTimer);
});

const toolCount = computed(() => props.groups.reduce((sum, group) => sum + group.runs.length, 0));
</script>

<template>
  <Collapsible v-model:open="open" class="group mb-1 w-full rounded-md border">
    <CollapsibleTrigger class="flex w-full items-center justify-between gap-2 px-2 py-1.5">
      <div class="flex min-w-0 items-center gap-1.5 text-muted-foreground">
        <Loader2Icon v-if="active" class="size-3.5 shrink-0 animate-spin" />
        <BrainIcon v-else class="size-3.5 shrink-0" />
        <Shimmer v-if="active" :duration="1" class="text-xs">
          {{ t("chat.process.running") }}
        </Shimmer>
        <template v-else>
          <span class="truncate text-xs">{{ t("chat.process.title") }}</span>
          <span v-if="toolCount > 0" class="shrink-0 text-xs">
            {{ t("chat.process.toolCount", { count: toolCount }) }}
          </span>
        </template>
      </div>
      <ChevronDownIcon
        class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
      />
    </CollapsibleTrigger>
    <CollapsibleContent
      class="data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-top-2 data-[state=open]:slide-in-from-top-2 outline-none data-[state=closed]:animate-out data-[state=open]:animate-in"
    >
      <div class="border-t px-1 pt-1">
        <template v-for="(group, gi) in groups" :key="gi">
          <!-- 思考原文:左侧细线与工具卡片区分;流式轮用 streaming 模式 -->
          <div
            v-if="group.thinking"
            class="mb-1 border-l-2 border-border/60 pl-2 text-muted-foreground text-xs"
          >
            <MessageResponse
              :content="group.thinking"
              :mode="group.thinkingStreaming ? 'streaming' : 'static'"
            />
          </div>
          <ChatToolCard
            v-for="(run, ri) in group.runs"
            :key="`${gi}-${ri}`"
            :run="run"
            class="border-0"
          />
        </template>
      </div>
    </CollapsibleContent>
  </Collapsible>
</template>
