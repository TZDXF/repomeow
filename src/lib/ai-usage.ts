/**
 * AI 用量统计:逐条记录每次 LLM 调用(任务类型 + token 消耗)到 SQLite(ai_usage_log 表)。
 * 三条链路:内置 API(ai.ts 采集)/ Rust 定时报告(scheduler.rs 直写)/
 * ACP agent 后端(wiki-generator.ts 按会话累计差分采集)。
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

/** 会话累计口径的 token 快照(ACP Usage 的核心字段) */
export interface AiUsageSnapshot {
  totalTokens: number;
  inputTokens: number;
  outputTokens: number;
  /** 缓存命中的输入 tokens;部分 agent 不上报 */
  cachedTokens?: number;
}

/**
 * ACP 的 Usage 是会话累计口径,相邻两次 prompt 差分出单次消耗。
 * 无 previous(会话首次 prompt)时累计值即本次消耗;
 * 任一字段出现回退(agent 侧重置计数等)时整体置缺,不伪造数值;
 * 缓存值仅在相邻两次快照都上报时才差分
 */
export function usageDelta(current: AiUsageSnapshot, previous?: AiUsageSnapshot): AiTokenUsage {
  if (!previous) {
    return {
      inputTokens: current.inputTokens,
      outputTokens: current.outputTokens,
      totalTokens: current.totalTokens,
      ...(current.cachedTokens !== undefined ? { cachedTokens: current.cachedTokens } : {}),
    };
  }
  const delta: AiTokenUsage = {
    inputTokens: current.inputTokens - previous.inputTokens,
    outputTokens: current.outputTokens - previous.outputTokens,
    totalTokens: current.totalTokens - previous.totalTokens,
  };
  const regressed = Object.values(delta).some((v) => v < 0);
  if (regressed) return {};
  if (current.cachedTokens !== undefined && previous.cachedTokens !== undefined) {
    delta.cachedTokens = current.cachedTokens - previous.cachedTokens;
  }
  return delta;
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
    },
  }).catch((e) => {
    console.warn("[ai-usage] record failed:", e);
  });
}
