import { beforeEach, describe, expect, it, vi } from "vitest";
import { cmd } from "./tauri";
import { AI_TASK_TYPES, mapAcpPromptUsage, recordAiUsage } from "./ai-usage";

vi.mock("./tauri", () => ({ cmd: vi.fn(() => Promise.resolve()) }));

describe("ACP prompt usage", () => {
  it("每次响应独立记录,后一请求用量较小也不做差分或置空", () => {
    const first = { totalTokens: 3000, inputTokens: 2400, outputTokens: 600 };
    const second = { totalTokens: 1500, inputTokens: 1000, outputTokens: 500 };

    expect(mapAcpPromptUsage(first)).toEqual(first);
    expect(mapAcpPromptUsage(second)).toEqual(second);
  });

  it("映射缓存读取 tokens", () => {
    expect(
      mapAcpPromptUsage({
        totalTokens: 2000,
        inputTokens: 1600,
        outputTokens: 400,
        cachedReadTokens: 900,
      }),
    ).toEqual({
      totalTokens: 2000,
      inputTokens: 1600,
      outputTokens: 400,
      cachedTokens: 900,
    });
  });
});

describe("recordAiUsage", () => {
  beforeEach(() => vi.mocked(cmd).mockClear());

  it("向后端传递缓存 tokens", () => {
    recordAiUsage({
      taskType: "wiki",
      model: "agent · model",
      usage: { inputTokens: 800, outputTokens: 200, totalTokens: 1000, cachedTokens: 300 },
      durationMs: 123,
    });

    expect(cmd).toHaveBeenCalledWith("record_ai_usage", {
      record: {
        taskType: "wiki",
        model: "agent · model",
        inputTokens: 800,
        outputTokens: 200,
        totalTokens: 1000,
        durationMs: 123,
        cachedTokens: 300,
      },
    });
  });
});

describe("AI_TASK_TYPES", () => {
  it("任务类型清单与后端 task_type 取值一致", () => {
    expect(AI_TASK_TYPES).toEqual(["commit", "report", "wiki"]);
  });
});
