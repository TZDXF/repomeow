<script setup lang="ts">
import { computed, ref, watch, type HTMLAttributes } from "vue";
import { useI18n } from "vue-i18n";
import { Collapsible } from "@/components/ui/collapsible";
import { ToolContent, ToolHeader, ToolInput, ToolOutput } from "@/components/ai-elements/tool";
import type { ToolState } from "@/components/ai-elements/tool/types";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { ChatToolRun } from "@/lib/chat";

/**
 * 项目问答的工具调用卡片:把 store 里的 ChatToolRun 映射到 ai-elements 的 Tool
 * 组件状态(ok=null 运行中,ok=true 完成,ok=false 失败),紧凑单行展示,
 * 展开后才看参数与结果。
 * 头部显示友好工具名(i18n chat.tools.<工具名>,未登记回退原始名)与主参数摘要。
 * ask 权限档下 pending/responding 的 run 内联展示确认区(说明 + 拒绝/允许本次),
 * 待确认时自动展开完整参数;不使用全局模态框。
 */
const props = defineProps<{ run: ChatToolRun; class?: HTMLAttributes["class"] }>();
const emit = defineEmits<{ respond: [payload: { id: string; allow: boolean }] }>();

const { t, te } = useI18n();

/** 权限确认区可见(pending 待确认 / responding 提交中) */
const awaiting = computed(
  () => props.run.permission === "pending" || props.run.permission === "responding",
);
const responding = computed(() => props.run.permission === "responding");
/** 已拒绝但仍等后端 toolResult 收尾 */
const deniedWaiting = computed(() => props.run.permission === "denied" && props.run.ok === null);

const state = computed<ToolState>(() => {
  if (awaiting.value) return "awaiting-permission";
  if (props.run.ok === null) return "input-available";
  return props.run.ok ? "output-available" : "output-error";
});

const title = computed(() => {
  const key = `chat.tools.${props.run.name}`;
  return te(key) ? t(key) : props.run.name;
});

const permissionResultKeys: Record<string, string> = {
  "Tool execution was denied by the user": "chat.toolPermission.denied",
  "Tool permission request timed out": "chat.toolPermission.timedOut",
  "Tool permission request was cancelled": "chat.toolPermission.cancelled",
};

const resultSummary = computed(() => {
  const key = permissionResultKeys[props.run.summary];
  return key ? t(key) : props.run.summary;
});

/** 每个工具挑最能说明调用意图的主参数,在头部行内展示(完整参数展开可见) */
const PRIMARY_ARGS: Record<string, string[]> = {
  sem_find: ["query"],
  sem_context: ["entity"],
  sem_relations: ["entity"],
  read_wiki: ["page_id"],
  add_custom_command: ["name"],
  generate_report: ["date_from", "date_to"],
  read_project_file: ["path"],
  set_wiki_model: ["model_id"],
};

const subtitle = computed(() => {
  const keys = PRIMARY_ARGS[props.run.name];
  if (!keys || props.run.args === null || typeof props.run.args !== "object") {
    return "";
  }
  const args = props.run.args as Record<string, unknown>;
  const parts = keys
    .map((key) => args[key])
    .filter(
      (value): value is string | number =>
        (typeof value === "string" && value !== "") || typeof value === "number",
    )
    .map(String);
  // 去重(如 date_from == date_to 的日报只显示一个日期)
  return [...new Set(parts)].join(" ~ ");
});

const hasArgs = computed(
  () =>
    props.run.args !== null &&
    typeof props.run.args === "object" &&
    Object.keys(props.run.args as Record<string, unknown>).length > 0,
);

// 待确认自动展开完整参数(responding 期间保持展开)
const open = ref(false);
watch(
  () => props.run.permission,
  (permission) => {
    if (permission === "pending" || permission === "responding") open.value = true;
  },
  { immediate: true },
);

function respond(allow: boolean) {
  if (props.run.permission !== "pending") return;
  emit("respond", { id: props.run.id, allow });
}
</script>

<template>
  <Collapsible
    v-model:open="open"
    :class="cn('group not-prose mb-1 w-full rounded-md border', props.class)"
  >
    <ToolHeader :state="state" :subtitle="subtitle" :title="title" />
    <ToolContent v-if="hasArgs || run.ok === false || run.summary || awaiting || deniedWaiting">
      <ToolInput v-if="hasArgs" :input="run.args" />
      <div v-if="awaiting" class="flex items-center justify-between gap-2 px-2 pt-0.5 pb-2">
        <p class="text-muted-foreground text-xs">{{ t("chat.toolPermission.explain") }}</p>
        <div class="flex shrink-0 items-center gap-1.5">
          <Button
            variant="outline"
            size="sm"
            class="h-6 px-2 text-xs"
            :disabled="responding"
            @click="respond(false)"
          >
            {{ t("chat.toolPermission.deny") }}
          </Button>
          <Button size="sm" class="h-6 px-2 text-xs" :disabled="responding" @click="respond(true)">
            {{ t("chat.toolPermission.allowOnce") }}
          </Button>
        </div>
      </div>
      <p v-else-if="deniedWaiting" class="text-muted-foreground px-2 pt-0.5 pb-2 text-xs">
        {{ t("chat.toolPermission.deniedWait") }}
      </p>
      <ToolOutput
        v-if="run.ok === false || run.summary"
        :error-text="run.ok === false ? resultSummary : undefined"
        :output="run.ok ? resultSummary : undefined"
      />
    </ToolContent>
  </Collapsible>
</template>
