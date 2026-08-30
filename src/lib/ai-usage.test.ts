import { describe, expect, it } from "vitest";
import { AI_TASK_TYPES } from "./ai-usage";

describe("AI_TASK_TYPES", () => {
  it("任务类型清单与后端 task_type 取值一致", () => {
    expect(AI_TASK_TYPES).toEqual(["commit", "report", "wiki", "chat"]);
  });
});
