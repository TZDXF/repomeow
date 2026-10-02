import { describe, expect, it } from "vitest";
import type { ReportSchedule } from "@/types";
import {
  buildScheduleFromForm,
  clampIntervalMinutes,
  createEmptyScheduleForm,
  isScheduleFormValid,
  projectNames,
  scheduleToForm,
  tagNames,
  weekdayModeOf,
  weeklyRangeLabel,
} from "./schedule-utils";

function makeSchedule(overrides: Partial<ReportSchedule> = {}): ReportSchedule {
  return {
    id: "s1",
    name: "日报",
    enabled: false,
    reportType: "daily",
    projectIds: [1, 2],
    tagIds: [3],
    authorMode: "all",
    timeOfDay: "18:30",
    weekdaysOnly: true,
    chineseWorkdayOnly: false,
    previousDay: false,
    weeklyWorkweek: false,
    weeklyStartWeekday: 2,
    weeklyEndWeekday: 6,
    lastRunAt: 123456,
    ...overrides,
  };
}

describe("scheduleToForm", () => {
  it("把持久化任务映射为表单,并拷贝数组", () => {
    const s = makeSchedule();
    const form = scheduleToForm(s);
    expect(form).toMatchObject({
      name: "日报",
      time: "18:30",
      reportType: "daily",
      previousDay: false,
      weekdayMode: "weekdays",
      weeklyWorkweek: false,
      weeklyStart: 2,
      weeklyEnd: 6,
      authorMode: "all",
      projectIds: [1, 2],
      tagIds: [3],
    });
    expect(form.projectIds).not.toBe(s.projectIds);
    expect(form.tagIds).not.toBe(s.tagIds);
  });

  it("weeklyStartWeekday/weeklyEndWeekday 为 0 时回退到 1/5", () => {
    const form = scheduleToForm(makeSchedule({ weeklyStartWeekday: 0, weeklyEndWeekday: 0 }));
    expect(form.weeklyStart).toBe(1);
    expect(form.weeklyEnd).toBe(5);
  });
});

describe("weekdayModeOf", () => {
  it("中国工作日优先于周一至周五", () => {
    expect(weekdayModeOf({ chineseWorkdayOnly: true, weekdaysOnly: true })).toBe("chineseWorkday");
    expect(weekdayModeOf({ chineseWorkdayOnly: false, weekdaysOnly: true })).toBe("weekdays");
    expect(weekdayModeOf({ chineseWorkdayOnly: false, weekdaysOnly: false })).toBe("everyday");
  });
});

describe("isScheduleFormValid", () => {
  it("项目或包含标签至少一项", () => {
    expect(isScheduleFormValid({ projectIds: [], tagIds: [] })).toBe(false);
    expect(isScheduleFormValid({ projectIds: [1], tagIds: [] })).toBe(true);
    expect(isScheduleFormValid({ projectIds: [], tagIds: [2] })).toBe(true);
  });
});

describe("buildScheduleFromForm", () => {
  it("新建:生成 id、默认启用、lastRunAt 为 null,名称去空白", () => {
    const form = { ...createEmptyScheduleForm(), name: "  晨会日报  ", projectIds: [7] };
    const s = buildScheduleFromForm(form, null);
    expect(s.id).toBeTruthy();
    expect(s.id).not.toBe("s1");
    expect(s.enabled).toBe(true);
    expect(s.lastRunAt).toBeNull();
    expect(s.name).toBe("晨会日报");
  });

  it("编辑:保留 id / enabled / lastRunAt", () => {
    const editing = makeSchedule();
    const form = { ...scheduleToForm(editing), name: "改名" };
    const s = buildScheduleFromForm(form, editing);
    expect(s.id).toBe("s1");
    expect(s.enabled).toBe(false);
    expect(s.lastRunAt).toBe(123456);
    expect(s.name).toBe("改名");
  });

  it("日报按 weekdayMode 置标志;周报强制归零", () => {
    const daily = buildScheduleFromForm(
      { ...createEmptyScheduleForm(), weekdayMode: "chineseWorkday", projectIds: [1] },
      null,
    );
    expect(daily.weekdaysOnly).toBe(false);
    expect(daily.chineseWorkdayOnly).toBe(true);

    const weekly = buildScheduleFromForm(
      {
        ...createEmptyScheduleForm(),
        reportType: "weekly",
        weekdayMode: "weekdays",
        projectIds: [1],
      },
      null,
    );
    expect(weekly.weekdaysOnly).toBe(false);
    expect(weekly.chineseWorkdayOnly).toBe(false);
  });
});

describe("weeklyRangeLabel", () => {
  const names = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

  it("按 1=周一..7=周日 映射范围", () => {
    expect(weeklyRangeLabel({ weeklyStartWeekday: 1, weeklyEndWeekday: 5 }, names)).toBe(
      "周一 ~ 周五",
    );
    expect(weeklyRangeLabel({ weeklyStartWeekday: 7, weeklyEndWeekday: 7 }, names)).toBe(
      "周日 ~ 周日",
    );
  });
});

describe("clampIntervalMinutes", () => {
  it("钳制到 1..1440 并取整,非法输入回退", () => {
    expect(clampIntervalMinutes(0)).toBe(10);
    expect(clampIntervalMinutes(NaN)).toBe(10);
    expect(clampIntervalMinutes(0.6)).toBe(1);
    expect(clampIntervalMinutes(2000)).toBe(1440);
    expect(clampIntervalMinutes(30.4)).toBe(30);
  });
});

describe("projectNames / tagNames", () => {
  it("按 id 反查名称并忽略缺失项", () => {
    const projects = [
      { id: 1, name: "甲" },
      { id: 2, name: "乙" },
    ];
    expect(projectNames([2, 99, 1], projects)).toEqual(["乙", "甲"]);
    expect(tagNames([1], [])).toEqual([]);
  });
});
