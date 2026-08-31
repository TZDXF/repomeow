import type { ComputedRef, InjectionKey } from "vue";
import { inject } from "vue";
import type { AiModelCost } from "@/lib/ai-config";

/**
 * context 组件套件的共享状态(provide/inject)。对齐 ai-elements-vue 官方实现,
 * 两点适配:① 不用 tokenlens,成本由 ai-config 模型元数据的 cost 费率($/M)估算;
 * ② usedTokens/maxTokens 允许 null(尚无用量/未知窗口),官方为必填 number。
 */

/** 最近一次回合的 token 明细(字段命名对齐 ai 包 LanguageModelUsage) */
export interface ContextUsage {
  inputTokens?: number;
  outputTokens?: number;
  reasoningTokens?: number;
  cachedInputTokens?: number;
}

/** 上下文构成估算(最近一次 LLM 请求,按部分计量;与 lib/chat 的 ChatContextBreakdown 同形) */
export interface ContextBreakdown {
  systemPrompt: number;
  tools: number;
  messages: number;
}

export interface ContextValue {
  /** 当前上下文占用 token 数(null = 尚无统计) */
  usedTokens: ComputedRef<number | null>;
  /** 模型上下文窗口(null = 元数据缺失) */
  maxTokens: ComputedRef<number | null>;
  /** 上一轮回合用量明细(undefined = 尚无完成的回合) */
  usage: ComputedRef<ContextUsage | undefined>;
  /** 模型费率($/M tokens),缺失时各行不展示成本 */
  cost: ComputedRef<AiModelCost | null | undefined>;
  /** 上下文构成估算(undefined/null = 尚无数据) */
  breakdown: ComputedRef<ContextBreakdown | null | undefined>;
  /** 平均缓存命中率 0~1(undefined/null = 尚无样本) */
  cacheHitRate: ComputedRef<number | null | undefined>;
}

export const ContextKey: InjectionKey<ContextValue> = Symbol("ai-elements-context");

export function useContextValue(): ContextValue {
  const ctx = inject(ContextKey);
  if (!ctx) throw new Error("Context 子组件必须放在 <Context> 内使用");
  return ctx;
}

/** 占用比例 0~1;用量或窗口缺失时返回 null */
export function contextPercent(used: number | null, max: number | null): number | null {
  if (used == null || !max || max <= 0) return null;
  return Math.min(1, used / max);
}

/** 百分比展示(最多一位小数),对齐官方 Intl percent 格式 */
export function formatContextPercent(ratio: number): string {
  return new Intl.NumberFormat("en-US", { style: "percent", maximumFractionDigits: 1 }).format(
    ratio,
  );
}

/** USD 金额展示:常规两位小数,极小金额(亚分)保留到 4 位避免恒为 $0.00 */
export function formatUsd(amount: number): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: amount > 0 && amount < 0.01 ? 4 : 2,
  }).format(amount);
}

/** 按 $/M 费率估算单项成本(tokens × rate / 1e6);费率缺失返回 undefined(该行不展示成本) */
export function estimateCost(
  tokens: number,
  ratePerMillion: number | undefined,
): number | undefined {
  if (ratePerMillion == null || tokens <= 0) return undefined;
  return (tokens / 1_000_000) * ratePerMillion;
}
