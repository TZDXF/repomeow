<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import {
  Tool,
  ToolContent,
  ToolHeader,
  ToolInput,
  ToolOutput,
} from "@/components/ai-elements/tool";
import type { ChatToolRun } from "@/lib/chat";

/**
 * 项目问答的工具调用卡片:把 store 里的 ChatToolRun 映射到 ai-elements 的 Tool
 * 组件状态(ok=null 运行中,ok=true 完成,ok=false 失败),紧凑单行展示,
 * 展开后才看参数与结果。
 * 头部显示友好工具名(i18n chat.tools.<工具名>,未登记回退原始名)与主参数摘要。
 */
const props = defineProps<{ run: ChatToolRun }>();

const { t, te } = useI18n();

const state = computed(() => {
  if (props.run.ok === null) return "input-available" as const;
  return props.run.ok ? ("output-available" as const) : ("output-error" as const);
});

const title = computed(() => {
  const key = `chat.tools.${props.run.name}`;
  return te(key) ? t(key) : props.run.name;
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
</script>

<template>
  <Tool class="mb-1 w-full">
    <ToolHeader :state="state" :subtitle="subtitle" :title="title" />
    <ToolContent v-if="hasArgs || run.ok === false || run.summary">
      <ToolInput v-if="hasArgs" :input="run.args" />
      <ToolOutput
        v-if="run.ok === false || run.summary"
        :error-text="run.ok === false ? run.summary : undefined"
        :output="run.ok ? run.summary : undefined"
      />
    </ToolContent>
  </Tool>
</template>
