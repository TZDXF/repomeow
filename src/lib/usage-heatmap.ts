/** AI 用量热力图网格构建:周列 × 7 行(周一在上),GitHub 贡献图风格 */

import { formatDate } from "@/lib/format";
import type { AiUsageDayStat } from "@/types";

/** 周列数:27 周 = 189 天,覆盖最近半年(后端按日聚合同步给到 190 天) */
export const USAGE_HEATMAP_WEEKS = 27;

export interface UsageHeatCell {
  /** 本地日期 YYYY-MM-DD */
  day: string;
  calls: number;
  totalTokens: number;
  /** 0 = 无用量,1-4 = 按窗口内最大日 tokens 分档的强度 */
  level: 0 | 1 | 2 | 3 | 4;
  /** 最后一列(本周)里超过今天的格子,渲染为占位 */
  future: boolean;
}

export interface UsageHeatmap {
  /** weeks[col][row]:col = 周列,row = 0..6(周一..周日) */
  weeks: UsageHeatCell[][];
  /** 月份标签:该列的某一行是当月 1 号(标签画在该列上方),month 为 1-12 */
  monthLabels: { col: number; month: number }[];
}

function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

/** 所在周的周一(本地时区) */
function mondayOfWeek(d: Date): Date {
  // getDay: 0=周日..6=周六 → 距周一的天数
  return addDays(d, -((d.getDay() + 6) % 7));
}

/** 日 tokens → 强度档(0-4);有调用但 provider 未返回 tokens 时按最低档记 1 */
export function usageLevel(
  calls: number,
  totalTokens: number,
  maxTokens: number,
): 0 | 1 | 2 | 3 | 4 {
  if (totalTokens <= 0) return calls > 0 ? 1 : 0;
  const ratio = totalTokens / Math.max(maxTokens, 1);
  if (ratio >= 0.75) return 4;
  if (ratio >= 0.5) return 3;
  if (ratio >= 0.25) return 2;
  return 1;
}

/**
 * 由按日聚合数据构建热力图网格。
 * 网格从「当前周往前数 WEEKS-1 周的周一」开始,到本周日结束;today 之后的格子标记为 future。
 */
export function buildUsageHeatmap(byDay: AiUsageDayStat[], today: Date = new Date()): UsageHeatmap {
  const statByDay = new Map(byDay.map((s) => [s.day, s]));
  const end = startOfDay(today);
  const start = addDays(mondayOfWeek(end), -7 * (USAGE_HEATMAP_WEEKS - 1));

  // 强度分档基于窗口内的最大日 tokens(窗口外数据不参与归一化)
  let maxTokens = 0;
  for (let i = 0; ; i++) {
    const d = addDays(start, i);
    if (d > end) break;
    const stat = statByDay.get(formatDate(d));
    if (stat && stat.totalTokens > maxTokens) maxTokens = stat.totalTokens;
  }

  const weeks: UsageHeatCell[][] = [];
  const monthLabels: { col: number; month: number }[] = [];
  for (let col = 0; col < USAGE_HEATMAP_WEEKS; col++) {
    const week: UsageHeatCell[] = [];
    for (let row = 0; row < 7; row++) {
      const d = addDays(start, col * 7 + row);
      const stat = statByDay.get(formatDate(d));
      const calls = stat?.calls ?? 0;
      const totalTokens = stat?.totalTokens ?? 0;
      week.push({
        day: formatDate(d),
        calls,
        totalTokens,
        level: usageLevel(calls, totalTokens, maxTokens),
        future: d > end,
      });
      if (d.getDate() === 1 && d <= end) {
        monthLabels.push({ col, month: d.getMonth() + 1 });
      }
    }
    weeks.push(week);
  }
  return { weeks, monthLabels };
}
