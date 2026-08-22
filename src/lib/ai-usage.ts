/**
 * AI 用量统计:逐条记录每次 LLM 调用(任务类型 + token 消耗)到 SQLite(ai_usage_log 表)。
 * 三条链路:内置 API(ai.ts 采集)/ Rust 定时报告(scheduler.rs 直写)/
 * ACP agent 后端(wiki-generator.ts 按单次 PromptResponse.usage 采集)。
 * 统计是旁路:任何记录失败都不影响生成主流程。
 */

import { cmd } from "@/lib/tauri";

/** 任务类型语义(与后端 ai_usage_log.task_type 取值一致) */
export type AiUsageTaskType = "commit" | "report" | "wiki";

/** 设置页筛选/徽标用的全部任务类型(i18n 键 settings.usage.tasks.<type>) */
export const AI_TASK_TYPES: AiUsageTaskType[] = ["commit", "report", "wiki"];

/** 一次调用的 token 用量;provider 未返回的字段为 undefined(落库 NULL,不计入汇总) */
export interface AiTokenUsage {
  inputTokens?: number;
  outputTokens?: number;
  totalTokens?: number;
  /** 缓存命中的输入 tokens(输入的子集);未上报时为 undefined */
  cachedTokens?: number;
}

/** ACP 单次 prompt 响应携带的 token 用量(unstable 字段) */
export interface AcpPromptUsage {
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  cachedReadTokens?: number;
}

/**
 * ACP PromptResponse.usage 表示本次 prompt 的消耗,每次响应独立记录。
 * 该字段仍属 unstable,不同 agent 可能缺报,但不能把相邻响应当作累计快照相减——
 * 否则后一请求用量较小时会被误判为计数回退并落成空记录。
 */
export function mapAcpPromptUsage(usage: AcpPromptUsage): AiTokenUsage {
  return {
    inputTokens: usage.inputTokens,
    outputTokens: usage.outputTokens,
    totalTokens: usage.totalTokens,
    ...(usage.cachedReadTokens !== undefined ? { cachedTokens: usage.cachedReadTokens } : {}),
  };
}

/** 记录一次用量(fire-and-forget:不 await、失败仅 console 警告,绝不影响生成主流程) */
export function recordAiUsage(entry: {
  taskType: AiUsageTaskType;
  model: string;
  usage?: AiTokenUsage;
  durationMs?: number;
}): void {
  // 结构体参数按 Tauri 约定以其参数名包裹(对应 Rust 端 record: AiUsageRecord)
  void cmd("record_ai_usage", {
    record: {
      taskType: entry.taskType,
      model: entry.model,
      inputTokens: entry.usage?.inputTokens ?? null,
      outputTokens: entry.usage?.outputTokens ?? null,
      totalTokens: entry.usage?.totalTokens ?? null,
      durationMs: entry.durationMs ?? null,
      cachedTokens: entry.usage?.cachedTokens ?? null,
    },
  }).catch((e) => {
    console.warn("[ai-usage] record failed:", e);
  });
}
