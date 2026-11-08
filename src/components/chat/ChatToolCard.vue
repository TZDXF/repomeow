<script setup lang="ts">
import { computed, ref, type HTMLAttributes } from "vue";
import { useI18n } from "vue-i18n";
import { Collapsible } from "@/components/ui/collapsible";
import { ToolContent, ToolHeader, ToolInput, ToolOutput } from "@/components/ai-elements/tool";
import type { ToolState } from "@/components/ai-elements/tool/types";
import { cn } from "@/lib/utils";
import type { ChatToolRun } from "@/lib/chat";

/**
 * 项目问答的工具调用卡片:把 store 里的 ChatToolRun 映射到 ai-elements 的 Tool
 * 组件状态(ok=null 运行中,ok=true 完成,ok=false 失败),紧凑单行展示,
 * 展开后才看参数与结果。
 * 头部显示友好工具名(i18n chat.tools.<工具名>,未登记回退原始名)与主参数摘要。
 */
const props = defineProps<{ run: ChatToolRun; class?: HTMLAttributes["class"] }>(),
  { t, te } = useI18n(),
  state = computed<ToolState>(() => {
    if (props.run.ok === null) {
      return "input-available";
    }
    return props.run.ok ? "output-available" : "output-error";
  }),
  title = computed(() => {
    const key = `chat.tools.${props.run.name}`;
    return te(key) ? t(key) : props.run.name;
  }),
  /** 每个工具挑最能说明调用意图的主参数,在头部行内展示(完整参数展开可见) */
  PRIMARY_ARGS: Record<string, string[]> = {
    sem_find: ["query"],
    sem_context: ["entity"],
    sem_relations: ["entity"],
    read_wiki: ["page_id"],
    add_custom_command: ["name"],
    update_custom_command: ["name"],
    delete_custom_command: ["name"],
    generate_report: ["date_from", "date_to"],
    read_project_file: ["path"],
    bash: ["command"],
    create_terminal_tab: ["command"],
    stop_terminal_tab: ["id"],
    restart_terminal_tab: ["id"],
    read_terminal_tab_output: ["id"],
    set_wiki_model: ["model_id"],
  },
  subtitle = computed(() => {
    const keys = PRIMARY_ARGS[props.run.name];
    if (!keys || props.run.args === null || typeof props.run.args !== "object") {
      return "";
    }
    const args = props.run.args as Record<string, unknown>,
      parts = keys
        .map((key) => args[key])
        .filter(
          (value): value is string | number =>
            (typeof value === "string" && value !== "") || typeof value === "number",
        )
        .map(String);
    // 去重(如 date_from == date_to 的日报只显示一个日期)
    return [...new Set(parts)].join(" ~ ");
  }),
  hasArgs = computed(
    () =>
      props.run.args !== null &&
      typeof props.run.args === "object" &&
      Object.keys(props.run.args as Record<string, unknown>).length > 0,
  ),
  open = ref(false);
</script>

<template>
  <Collapsible
    v-model:open="open"
    :class="cn('group not-prose mb-1 w-full rounded-md border', props.class)"
  >
    <ToolHeader :state="state" :subtitle="subtitle" :title="title" />
    <ToolContent v-if="hasArgs || run.ok === false || run.summary">
      <ToolInput v-if="hasArgs" :input="run.args" />
      <ToolOutput
        v-if="run.ok === false || run.summary"
        :error-text="run.ok === false ? run.summary : undefined"
        :output="run.ok ? run.summary : undefined"
      />
    </ToolContent>
  </Collapsible>
</template>
