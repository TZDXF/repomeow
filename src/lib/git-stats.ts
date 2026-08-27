/** 提交统计(git_project_stats)的前端聚合与图表数据构建 */
import { formatDate } from "@/lib/format";
import type { GitDayStat } from "@/types";

/** 提交日历热力图周列数:53 周 ≈ 最近 12 个月(GitHub 贡献图一整年) */
export const COMMIT_CALENDAR_WEEKS = 53;

/** 提交趋势/变更趋势展示的周数(取最近一年) */
export const TREND_WEEKS = 52;

export interface CommitCalendarCell {
  /** 本地日期 YYYY-MM-DD */
  day: string;
  count: number;
  /** 0 = 无提交,1-4 = 按窗口内最大日提交数分档 */
  level: 0 | 1 | 2 | 3 | 4;
  /** 最后一列(本周)里超过今天的格子,渲染为占位 */
  future: boolean;
}

export interface CommitCalendar {
  /** weeks[col][row]:col = 周列,row = 0..6(周一..周日) */
  weeks: CommitCalendarCell[][];
  /** 月份标签:该列的某一行是当月 1 号(标签画在该列上方),month 为 1-12 */
  monthLabels: { col: number; month: number }[];
}

/** 一周的提交聚合;day 为周一日期 YYYY-MM-DD */
export interface GitWeekStat {
  day: string;
  count: number;
  additions: number;
  deletions: number;
}

/**
 * 后端 byDay 的 t → 日期串 "YYYY-MM-DD"。
 * t 的 UTC 日期即提交者本地日期(仅作日历标识),故用 UTC 字段还原,避免浏览者时区串日
 */
export function dayKeyOf(t: number): string {
  const d = new Date(t * 1000);
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  return `${d.getUTCFullYear()}-${m}-${day}`;
}

/** 提交数 → 强度档(0-4),阈值与设置页用量热力图一致(25%/50%/75% 分档) */
export function heatLevel(count: number, max: number): 0 | 1 | 2 | 3 | 4 {
  if (count <= 0) return 0;
  const ratio = count / Math.max(max, 1);
  if (ratio >= 0.75) return 4;
  if (ratio >= 0.5) return 3;
  if (ratio >= 0.25) return 2;
  return 1;
}

/**
 * 由按日聚合构建提交日历热力图网格(周列 × 周一~周日行)。
 * 网格从「当前周往前数 WEEKS-1 周的周一」开始,到本周日结束;today 之后的格子标记为 future。
 * 强度分档基于窗口内的最大日提交数(窗口外数据不参与归一化)。
 */
export function buildCommitCalendar(byDay: GitDayStat[], today: Date = new Date()): CommitCalendar {
  const countByDay = new Map(byDay.map((d) => [dayKeyOf(d.t), d.count]));
  const end = startOfDay(today);
  const start = addDays(mondayOfWeek(end), -7 * (COMMIT_CALENDAR_WEEKS - 1));

  let maxCount = 0;
  for (let i = 0; ; i++) {
    const d = addDays(start, i);
    if (d > end) break;
    const count = countByDay.get(formatDate(d)) ?? 0;
    if (count > maxCount) maxCount = count;
  }

  const weeks: CommitCalendarCell[][] = [];
  const monthLabels: { col: number; month: number }[] = [];
  for (let col = 0; col < COMMIT_CALENDAR_WEEKS; col++) {
    const week: CommitCalendarCell[] = [];
    for (let row = 0; row < 7; row++) {
      const d = addDays(start, col * 7 + row);
      const count = countByDay.get(formatDate(d)) ?? 0;
      week.push({
        day: formatDate(d),
        count,
        level: heatLevel(count, maxCount),
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

/** 按日聚合 → 按周(周一起)聚合,按周升序;取最近 limit 周(默认一年) */
export function aggregateWeeks(byDay: GitDayStat[], limit = TREND_WEEKS): GitWeekStat[] {
  const weeks = new Map<string, GitWeekStat>();
  for (const d of byDay) {
    const date = new Date(d.t * 1000);
    // 提交者本地日期存在 UTC 刻度上,故用 UTC 字段推算周一
    const weekday = (date.getUTCDay() + 6) % 7;
    const monday = new Date(
      Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate() - weekday),
    );
    const key = dayKeyOf(monday.getTime() / 1000);
    let week = weeks.get(key);
    if (!week) {
      week = { day: key, count: 0, additions: 0, deletions: 0 };
      weeks.set(key, week);
    }
    week.count += d.count;
    week.additions += d.additions;
    week.deletions += d.deletions;
  }
  return [...weeks.values()].sort((a, b) => a.day.localeCompare(b.day)).slice(-limit);
}

/** 代码变更 K 线蜡烛:实体为 0~净变更,影线覆盖 −deletions~additions */
export interface ChurnCandle {
  /** 周起始日期 YYYY-MM-DD */
  day: string;
  /** 开盘,恒为 0(变更从零点出发) */
  open: number;
  /** 收盘,净变更 = additions − deletions */
  close: number;
  /** 影线最低 = −deletions */
  low: number;
  /** 影线最高 = additions */
  high: number;
  additions: number;
  deletions: number;
}

/** 按日聚合 → 每周一根 K 线蜡烛(净新增为阳线,净减少为阴线),按周升序取最近 limit 周 */
export function buildChurnCandles(byDay: GitDayStat[], limit = TREND_WEEKS): ChurnCandle[] {
  return aggregateWeeks(byDay, limit).map((w) => ({
    day: w.day,
    open: 0,
    close: w.additions - w.deletions,
    low: -w.deletions,
    high: w.additions,
    additions: w.additions,
    deletions: w.deletions,
  }));
}

/** Top N + 其余归并为一项(merge 由调用方求和);不足 N 项时原样返回 */
export function topWithOther<T>(items: T[], limit: number, merge: (rest: T[]) => T): T[] {
  if (items.length <= limit) return items;
  return [...items.slice(0, limit), merge(items.slice(limit))];
}

/** 作息热力图取值:weekdayHour 为 7*24 行主序(行 = 周一..周日) */
export function weekdayHourAt(weekdayHour: number[], weekday: number, hour: number): number {
  return weekdayHour[weekday * 24 + hour] ?? 0;
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
