import { afterEach, describe, expect, it, vi } from "vitest";
import { scheduleHourly } from "./hourly-scheduler";

afterEach(() => vi.useRealTimers());

describe("scheduleHourly", () => {
  it("在下一个整点执行,随后每个整点执行,停止后不再执行", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 19, 10, 35));
    const callback = vi.fn();
    const stop = scheduleHourly(callback);
    vi.advanceTimersByTime(25 * 60_000 - 1);
    expect(callback).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(callback).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(60 * 60_000);
    expect(callback).toHaveBeenCalledTimes(2);
    stop();
    vi.advanceTimersByTime(60 * 60_000);
    expect(callback).toHaveBeenCalledTimes(2);
  });

  it("整点启动时等待下一小时,跨午夜正确调度", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 19, 23));
    const callback = vi.fn();
    const stop = scheduleHourly(callback);
    vi.advanceTimersByTime(60 * 60_000 - 1);
    expect(callback).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(callback).toHaveBeenCalledOnce();
    expect(new Date().getHours()).toBe(0);
    stop();
  });
});
