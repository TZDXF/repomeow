import { describe, expect, it } from "vitest";
import type { AiUsageDayStat } from "@/types";
import { formatDate, parseDateStr } from "./format";
import { buildUsageHeatmap, USAGE_HEATMAP_WEEKS, usageLevel } from "./usage-heatmap";

function stat(day: string, totalTokens: number, calls = 1): AiUsageDayStat {
  return { day, calls, inputTokens: 0, outputTokens: 0, totalTokens, cachedTokens: 0 };
}

describe("usageLevel", () => {
  it("无调用无 tokens 为 0 档,其余按最大值的 25/50/75% 分档", () => {
    expect(usageLevel(0, 0, 100)).toBe(0);
    expect(usageLevel(1, 1, 100)).toBe(1);
    expect(usageLevel(1, 25, 100)).toBe(2);
    expect(usageLevel(1, 50, 100)).toBe(3);
    expect(usageLevel(1, 75, 100)).toBe(4);
    expect(usageLevel(1, 100, 100)).toBe(4);
  });

  it("有调用但 provider 未返回 tokens 时按最低档 1 处理", () => {
    expect(usageLevel(3, 0, 100)).toBe(1);
  });

  it("maxTokens 为 0(窗口内全空)时不除零", () => {
    expect(usageLevel(0, 0, 0)).toBe(0);
  });
});

describe("buildUsageHeatmap", () => {
  // 2026-08-27 是周四
  const today = new Date(2026, 7, 27);

  it("生成 27 列 × 7 行,首列从周一开始,最后一列包含今天", () => {
    const { weeks } = buildUsageHeatmap([], today);
    expect(weeks).toHaveLength(USAGE_HEATMAP_WEEKS);
    for (const week of weeks) expect(week).toHaveLength(7);

    const first = parseDateStr(weeks[0][0].day);
    expect(first.getDay()).toBe(1);
    const lastWeek = weeks[USAGE_HEATMAP_WEEKS - 1];
    expect(lastWeek.some((c) => c.day === "2026-08-27")).toBe(true);
    // 网格连续无跳空
    const span = (parseDateStr(lastWeek[6].day).getTime() - first.getTime()) / 86_400_000;
    expect(span).toBe(USAGE_HEATMAP_WEEKS * 7 - 1);
  });

  it("今天之后的格子标记为 future,今天及之前不是", () => {
    const { weeks } = buildUsageHeatmap([], today);
    const lastWeek = weeks[USAGE_HEATMAP_WEEKS - 1];
    for (const cell of lastWeek) {
      expect(cell.future).toBe(cell.day > "2026-08-27");
    }
    expect(weeks.flat().filter((c) => c.future)).toHaveLength(3); // 周五/六/日
  });

  it("按日数据落到对应格子并按窗口最大值分档,无记录的日期为 0 档", () => {
    // 网格起点:2026-08-24(本周周一)往前 26 周 = 2026-02-23
    const { weeks } = buildUsageHeatmap(
      [stat("2026-08-27", 100), stat("2026-02-23", 50), stat("2026-02-24", 10)],
      today,
    );
    const flat = weeks.flat();
    expect(flat.find((c) => c.day === "2026-08-27")).toMatchObject({
      calls: 1,
      totalTokens: 100,
      level: 4,
    });
    expect(flat.find((c) => c.day === "2026-02-23")).toMatchObject({ level: 3 });
    expect(flat.find((c) => c.day === "2026-02-24")).toMatchObject({ level: 1 });
    expect(flat.find((c) => c.day === "2026-02-25")).toMatchObject({
      calls: 0,
      totalTokens: 0,
      level: 0,
    });
  });

  it("窗口外的数据不参与最大值归一化", () => {
    const { weeks } = buildUsageHeatmap([], today);
    const oldest = weeks[0][0].day;
    // 窗口外更早一天放一个超大值,窗口内一个小值仍应拿到 4 档
    const d0 = parseDateStr(oldest);
    const outside = formatDate(new Date(d0.getFullYear(), d0.getMonth(), d0.getDate() - 1));
    const { weeks: w2 } = buildUsageHeatmap(
      [stat(outside, 1_000_000), stat("2026-08-27", 10)],
      today,
    );
    expect(w2.flat().find((c) => c.day === "2026-08-27")?.level).toBe(4);
  });

  it("月份标签落在包含当月 1 号的列,且按时间递增", () => {
    const { weeks, monthLabels } = buildUsageHeatmap([], today);
    // 2026-02-23 ~ 2026-08-30 窗口内包含 3/1、4/1、5/1、6/1、7/1、8/1
    expect(monthLabels.map((l) => l.month)).toEqual([3, 4, 5, 6, 7, 8]);
    for (const { col, month } of monthLabels) {
      const suffix = `-${String(month).padStart(2, "0")}-01`;
      expect(weeks[col].some((c) => c.day.endsWith(suffix))).toBe(true);
    }
  });

  it("今天恰为周一/周日时网格边界正确", () => {
    const monday = new Date(2026, 7, 24);
    const { weeks: wMon } = buildUsageHeatmap([], monday);
    expect(wMon[USAGE_HEATMAP_WEEKS - 1][0].day).toBe("2026-08-24");
    expect(wMon[USAGE_HEATMAP_WEEKS - 1].filter((c) => c.future)).toHaveLength(6);

    const sunday = new Date(2026, 7, 30);
    const { weeks: wSun } = buildUsageHeatmap([], sunday);
    expect(wSun[USAGE_HEATMAP_WEEKS - 1][6].day).toBe("2026-08-30");
    expect(wSun.flat().every((c) => !c.future)).toBe(true);
  });
});
