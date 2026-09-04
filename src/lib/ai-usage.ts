/** AI 用量任务类型。采集和落库均在 Rust，前端只用于筛选与展示。 */

/** 任务类型语义(与后端 ai_usage_log.task_type 取值一致) */
export type AiUsageTaskType = "commit" | "report" | "wiki" | "chat" | "translate";

/** 设置页筛选/徽标用的全部任务类型(i18n 键 settings.usage.tasks.<type>) */
export const AI_TASK_TYPES: AiUsageTaskType[] = ["commit", "report", "wiki", "chat", "translate"];
