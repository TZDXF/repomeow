import { describe, expect, it } from "vitest";
import {
  aggregateWeeks,
  buildChurnCandles,
  buildCommitCalendar,
  COMMIT_CALENDAR_WEEKS,
  dayKeyOf,
  heatLevel,
  topWithOther,
  weekdayHourAt,
} from "@/lib/git-stats";
import type { GitDayStat } from "@/types";

/** 由 "YYYY-MM-DD" 构造 GitDayStat(t 取该日 UTC 零点) */
function day(dayStr: string, count: number, additions = 0, deletions = 0): GitDayStat {
  return { t: Date.parse(`${dayStr}T00:00:00Z`) / 1000, count, additions, deletions };
}

describe("dayKeyOf", () => {
  it("以 UTC 字段还原日期,不受浏览者时区影响", () => {
    expect(dayKeyOf(0)).toBe("1970-01-01");
    expect(dayKeyOf(354_600)).toBe("1970-01-05");
    expect(dayKeyOf(Date.parse("2026-08-27T00:00:00Z") / 1000)).toBe("2026-08-27");
  });
});

describe("heatLevel", () => {
  it("按最大值分档,0 提交恒为 0 档", () => {
    expect(heatLevel(0, 10)).toBe(0);
    expect(heatLevel(1, 100)).toBe(1);
    expect(heatLevel(25, 100)).toBe(2);
    expect(heatLevel(50, 100)).toBe(3);
    expect(heatLevel(75, 100)).toBe(4);
    expect(heatLevel(100, 100)).toBe(4);
  });

  it("max 为 0 时不除零", () => {
    expect(heatLevel(0, 0)).toBe(0);
    expect(heatLevel(3, 0)).toBe(4);
  });
});

describe("buildCommitCalendar", () => {
  const today = new Date(2026, 7, 27); // 2026-08-27 周四(本地时区)

  it("生成 53 周 × 7 行,含未来占位格", () => {
    const { weeks } = buildCommitCalendar([], today);
    expect(weeks).toHaveLength(COMMIT_CALENDAR_WEEKS);
    for (const week of weeks) expect(week).toHaveLength(7);
    // 今天之后的格子标记 future;今天本身不标记
    const flat = weeks.flat();
    expect(flat.find((c) => c.day === "2026-08-27")?.future).toBe(false);
    expect(flat.filter((c) => c.future).every((c) => c.day > "2026-08-27")).toBe(true);
  });

  it("数据落到对应日期格,强度按窗口内最大值归一化", () => {
    // 网格起点:本周周一往前 52 周
    const { weeks } = buildCommitCalendar([day("2026-08-24", 4), day("2026-08-25", 2)], today);
    const flat = weeks.flat();
    expect(flat.find((c) => c.day === "2026-08-24")).toMatchObject({ count: 4, level: 4 });
    expect(flat.find((c) => c.day === "2026-08-25")).toMatchObject({ count: 2, level: 3 });
    // 窗口外的历史数据不进入网格
    const old = buildCommitCalendar([day("2000-01-03", 99)], today);
    expect(old.weeks.flat().reduce((s, c) => s + c.count, 0)).toBe(0);
  });

  it("月份标签标在含当月 1 号的周列", () => {
    const { weeks, monthLabels } = buildCommitCalendar([], today);
    for (const { col, month } of monthLabels) {
      const days = weeks[col].map((c) => c.day);
      expect(days.some((d) => d.endsWith("-01") && Number(d.slice(5, 7)) === month)).toBe(true);
    }
  });
});

describe("aggregateWeeks", () => {
  it("按周一起归并并求和,按周升序", () => {
    // 2026-08-24 周一;08-22/08-23 属于上一周(周一 08-17)
    const weeks = aggregateWeeks([
      day("2026-08-24", 1, 10, 1),
      day("2026-08-26", 2, 20, 2),
      day("2026-08-23", 3, 30, 3),
    ]);
    expect(weeks).toEqual([
      { day: "2026-08-17", count: 3, additions: 30, deletions: 3 },
      { day: "2026-08-24", count: 3, additions: 30, deletions: 3 },
    ]);
  });

  it("超出窗口时只保留最近 limit 周", () => {
    const days = [
      day("2025-08-25", 5), // 恰好一年前的周
      day("2026-08-24", 1),
    ];
    expect(aggregateWeeks(days, 1)).toEqual([
      { day: "2026-08-24", count: 1, additions: 0, deletions: 0 },
    ]);
  });
});

describe("buildChurnCandles", () => {
  it("每周一根蜡烛:实体 0~净变更,影线 −deletions~additions", () => {
    const candles = buildChurnCandles([
      day("2026-08-24", 1, 100, 30), // 周一
      day("2026-08-26", 1, 20, 40), // 同周,合计 120 增 70 删,净 +50
      day("2026-08-31", 1, 10, 80), // 下一周,净 −70
    ]);
    expect(candles).toEqual([
      { day: "2026-08-24", open: 0, close: 50, low: -70, high: 120, additions: 120, deletions: 70 },
      { day: "2026-08-31", open: 0, close: -70, low: -80, high: 10, additions: 10, deletions: 80 },
    ]);
  });

  it("空输入返回空数组", () => {
    expect(buildChurnCandles([])).toEqual([]);
  });
});

describe("topWithOther", () => {
  const sum = (rest: number[]) => rest.reduce((a, b) => a + b, 0);

  it("不足 limit 原样返回,超出部分由 merge 归并", () => {
    expect(topWithOther([3, 2, 1], 3, sum)).toEqual([3, 2, 1]);
    expect(topWithOther([5, 4, 3, 2, 1], 2, sum)).toEqual([5, 4, 6]);
  });
});

describe("weekdayHourAt", () => {
  it("按 7*24 行主序取值,越界回退 0", () => {
    const grid = Array.from({ length: 168 }, (_, i) => i);
    expect(weekdayHourAt(grid, 0, 0)).toBe(0);
    expect(weekdayHourAt(grid, 1, 0)).toBe(24);
    expect(weekdayHourAt(grid, 6, 23)).toBe(167);
    expect(weekdayHourAt([], 6, 23)).toBe(0);
  });
});
