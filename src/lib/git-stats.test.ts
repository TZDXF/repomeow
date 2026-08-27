import { describe, expect, it } from "vitest";
import {
  aggregateWeeks,
  buildCommitCalendar,
  buildCumulativeCommits,
  COMMIT_CALENDAR_WEEKS,
  dayKeyOf,
  filterLanguageFileTypes,
  heatLevel,
  topWithOther,
  weekdayHourAt,
} from "@/lib/git-stats";
import type { GitDayStat, GitFileTypeStat } from "@/types";

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

describe("buildCumulativeCommits", () => {
  it("按日升序逐点累加,保留日期与原时刻", () => {
    const points = buildCumulativeCommits([
      day("2026-08-24", 3),
      day("2026-08-26", 2),
      day("2026-09-01", 5),
    ]);
    expect(points).toEqual([
      { t: Date.parse("2026-08-24T00:00:00Z") / 1000, day: "2026-08-24", total: 3 },
      { t: Date.parse("2026-08-26T00:00:00Z") / 1000, day: "2026-08-26", total: 5 },
      { t: Date.parse("2026-09-01T00:00:00Z") / 1000, day: "2026-09-01", total: 10 },
    ]);
  });

  it("空输入返回空数组", () => {
    expect(buildCumulativeCommits([])).toEqual([]);
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

describe("filterLanguageFileTypes", () => {
  const ft = (ext: string): GitFileTypeStat => ({ ext, files: 1, bytes: 100 });

  it("只保留语言类文件,排除图片/配置/数据与未知扩展名", () => {
    const filtered = filterLanguageFileTypes(
      ["rs", "vue", "md", "dockerfile", "png", "icns", "yaml", "lock", "json", "svg", "xyz"].map(
        ft,
      ),
    );
    expect(filtered.map((f) => f.ext)).toEqual(["rs", "vue", "md", "dockerfile"]);
  });

  it("排除后端的 (other) 归并键;空输入返回空数组", () => {
    expect(filterLanguageFileTypes([ft("(other)"), ft("noext")])).toEqual([]);
    expect(filterLanguageFileTypes([])).toEqual([]);
  });
});
