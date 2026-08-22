import { describe, expect, it } from "vitest";
import { AI_TASK_TYPES, usageDelta } from "./ai-usage";

describe("usageDelta", () => {
  const first = { totalTokens: 1500, inputTokens: 1000, outputTokens: 500 };

  it("会话首次 prompt:无 previous,累计值即本次消耗", () => {
    expect(usageDelta(first)).toEqual(first);
  });

  it("相邻两次差分出单次消耗", () => {
    expect(usageDelta({ totalTokens: 3000, inputTokens: 2400, outputTokens: 600 }, first)).toEqual({
      totalTokens: 1500,
      inputTokens: 1400,
      outputTokens: 100,
    });
  });

  it("任一字段回退(agent 重置计数)时整体置缺,不伪造数值", () => {
    expect(usageDelta({ totalTokens: 3200, inputTokens: 900, outputTokens: 700 }, first)).toEqual(
      {},
    );
  });

  it("累计值未变化时差分为 0(合法)", () => {
    expect(usageDelta(first, first)).toEqual({ totalTokens: 0, inputTokens: 0, outputTokens: 0 });
  });

  it("缓存 tokens:会话首次 prompt 直接透传", () => {
    expect(
      usageDelta({ totalTokens: 1000, inputTokens: 800, outputTokens: 200, cachedTokens: 300 }),
    ).toEqual({ totalTokens: 1000, inputTokens: 800, outputTokens: 200, cachedTokens: 300 });
  });

  it("缓存 tokens:相邻两次都上报才差分,缺报则省略", () => {
    const cur = { totalTokens: 2000, inputTokens: 1600, outputTokens: 400, cachedTokens: 900 };
    const prev = { totalTokens: 1000, inputTokens: 800, outputTokens: 200, cachedTokens: 300 };
    expect(usageDelta(cur, prev)).toEqual({
      totalTokens: 1000,
      inputTokens: 800,
      outputTokens: 200,
      cachedTokens: 600,
    });
    const noCachedPrev = { totalTokens: 1000, inputTokens: 800, outputTokens: 200 };
    expect(usageDelta(cur, noCachedPrev).cachedTokens).toBeUndefined();
  });

  it("任务类型清单与后端 task_type 取值一致", () => {
    expect(AI_TASK_TYPES).toEqual(["commit", "report", "wiki"]);
  });
});
