<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  CheckCircleIcon,
  ChevronDownIcon,
  Loader2Icon,
  WrenchIcon,
  XCircleIcon,
} from "@lucide/vue";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { useI18n } from "vue-i18n";
import type { ChatToolRun } from "@/lib/chat";
import ChatToolCard from "./ChatToolCard.vue";

/**
 * 连续工具调用的折叠分组:单个调用直接渲染紧凑卡片;多个调用折叠为
 * 「调用了 N 个工具」一行摘要,进行中自动展开,全部结束后延迟自动收起
 * 一次(用户手动展开后不再干预)。
 */
const props = defineProps<{ runs: ChatToolRun[] }>();

const { t } = useI18n();

const running = computed(() => props.runs.some((run) => run.ok === null));
const failed = computed(() => !running.value && props.runs.some((run) => run.ok === false));

const open = ref(false);
const hasAutoClosed = ref(false);

watch(
  running,
  (isRunning) => {
    if (isRunning) {
      open.value = true;
      hasAutoClosed.value = false;
    } else if (open.value && !hasAutoClosed.value) {
      hasAutoClosed.value = true;
      setTimeout(() => {
        open.value = false;
      }, 800);
    }
  },
  { immediate: true },
);
</script>

<template>
  <ChatToolCard v-if="runs.length === 1" :run="runs[0]!" />
  <Collapsible v-else v-model:open="open" class="group mb-1 w-full rounded-md border">
    <CollapsibleTrigger class="flex w-full items-center justify-between gap-2 px-2 py-1.5">
      <div class="flex min-w-0 items-center gap-1.5 text-muted-foreground">
        <WrenchIcon class="size-3.5 shrink-0" />
        <span class="truncate text-xs">{{ t("chat.toolGroup", { count: runs.length }) }}</span>
        <Loader2Icon v-if="running" class="size-3.5 shrink-0 animate-spin" />
        <XCircleIcon v-else-if="failed" class="size-3.5 shrink-0 text-red-600" />
        <CheckCircleIcon v-else class="size-3.5 shrink-0 text-green-600" />
      </div>
      <ChevronDownIcon
        class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180"
      />
    </CollapsibleTrigger>
    <CollapsibleContent
      class="data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-top-2 data-[state=open]:slide-in-from-top-2 outline-none data-[state=closed]:animate-out data-[state=open]:animate-in"
    >
      <div class="border-t px-1 pt-1">
        <ChatToolCard v-for="(run, index) in runs" :key="index" :run="run" class="border-0" />
      </div>
    </CollapsibleContent>
  </Collapsible>
</template>
