// ── AI 用量统计 ─────────────────────────────────────────────────────────

/** ai_usage_log 的一行(明细日志;token 可为 null:provider 未返回 usage) */
export interface AiUsageEntry {
  id: number;
  createdAt: number;
  taskType: string;
  model: string;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  durationMs: number | null;
  /** 缓存命中的输入 tokens(输入的子集);未上报为 null */
  cachedTokens: number | null;
}

/** 按任务类型聚合的一行(get_ai_usage_summary 返回) */
export interface AiUsageTaskStat {
  taskType: string;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedTokens: number;
}

/** 按日聚合的一行(day 为本机时区 YYYY-MM-DD,最近约半年倒序) */
export interface AiUsageDayStat {
  day: string;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  cachedTokens: number;
}

/** 用量汇总统计(get_ai_usage_summary 返回;SUM 忽略 token 为 null 的行) */
export interface AiUsageSummary {
  totalCalls: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalTokens: number;
  totalCachedTokens: number;
  byTask: AiUsageTaskStat[];
  byDay: AiUsageDayStat[];
}
