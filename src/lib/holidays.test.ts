import { describe, expect, it, vi } from "vitest";
import type { ComposerTranslation } from "vue-i18n";
import {
  getHolidayDayClass,
  getHolidayDayTitle,
  localizedHolidayName,
  type HolidayDayContext,
} from "@/lib/holidays";

// Tauri invoke 在 Node 测试环境不可用,模块加载前注册空 mock(本文件不触发真实调用)
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

// t 桩:直接回传 key,便于断言回退到哪个词条
const t = ((key: string) => key) as unknown as ComposerTranslation;

function makeCtx(over: Partial<HolidayDayContext> = {}): HolidayDayContext {
  return {
    holidaySet: new Set<string>(),
    workdaySet: new Set<string>(),
    holidayNames: {},
    workdayNames: {},
    ...over,
  };
}

describe("localizedHolidayName", () => {
  const name = { en: "Spring Festival", zh: "春节" };

  it("中文环境取中文名,其余语言取英文名", () => {
    expect(localizedHolidayName(name, "zh-CN")).toBe("春节");
    expect(localizedHolidayName(name, "en-US")).toBe("Spring Festival");
  });

  it("无名称数据返回 undefined", () => {
    expect(localizedHolidayName(undefined, "zh-CN")).toBeUndefined();
  });
});

describe("getHolidayDayTitle", () => {
  it("节假日优先展示真实节日名(按语言)", () => {
    const ctx = makeCtx({
      holidaySet: new Set(["2026-02-15"]),
      holidayNames: { "2026-02-15": { en: "Spring Festival", zh: "春节" } },
    });
    expect(getHolidayDayTitle("2026-02-15", false, ctx, "zh-CN", t)).toBe("春节");
    expect(getHolidayDayTitle("2026-02-15", false, ctx, "en-US", t)).toBe("Spring Festival");
  });

  it("节假日无名称数据时回退通用词条", () => {
    const ctx = makeCtx({ holidaySet: new Set(["2026-02-15"]) });
    expect(getHolidayDayTitle("2026-02-15", false, ctx, "zh-CN", t)).toBe("reportHistory.holiday");
  });

  it("调休补班附带所补节日名,无名称时仅通用词条", () => {
    const ctx = makeCtx({
      workdaySet: new Set(["2026-02-14"]),
      workdayNames: { "2026-02-14": { en: "Spring Festival", zh: "春节" } },
    });
    expect(getHolidayDayTitle("2026-02-14", true, ctx, "zh-CN", t)).toBe(
      "reportHistory.makeupWorkday · 春节",
    );
    expect(
      getHolidayDayTitle(
        "2026-02-14",
        true,
        makeCtx({ workdaySet: new Set(["2026-02-14"]) }),
        "zh-CN",
        t,
      ),
    ).toBe("reportHistory.makeupWorkday");
  });

  it("普通周末与普通工作日", () => {
    expect(getHolidayDayTitle("2026-08-22", true, makeCtx(), "zh-CN", t)).toBe(
      "reportHistory.weekend",
    );
    expect(getHolidayDayTitle("2026-08-20", false, makeCtx(), "zh-CN", t)).toBeUndefined();
  });
});

describe("getHolidayDayClass", () => {
  it("节假日红 > 调休绿 > 普通周末淡红", () => {
    expect(getHolidayDayClass("h", true, makeCtx({ holidaySet: new Set(["h"]) }))).toBe(
      "report-calendar-holiday",
    );
    expect(getHolidayDayClass("w", true, makeCtx({ workdaySet: new Set(["w"]) }))).toBe(
      "report-calendar-makeup",
    );
    expect(getHolidayDayClass("s", true, makeCtx())).toBe("report-calendar-weekend");
    expect(getHolidayDayClass("d", false, makeCtx())).toBe("");
  });
});
