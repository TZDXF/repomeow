<script setup lang="ts">
import { computed } from "vue";
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
 */
const props = defineProps<{ run: ChatToolRun }>();

const state = computed(() => {
  if (props.run.ok === null) return "input-available" as const;
  return props.run.ok ? ("output-available" as const) : ("output-error" as const);
});
</script>

<template>
  <Tool class="mb-1 w-full">
    <ToolHeader :state="state" :title="run.name" />
    <ToolContent>
      <ToolInput :input="run.args" />
      <ToolOutput
        v-if="run.ok === false || run.summary"
        :error-text="run.ok === false ? run.summary : undefined"
        :output="run.ok ? run.summary : undefined"
      />
    </ToolContent>
  </Tool>
</template>
