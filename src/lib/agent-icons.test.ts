import { describe, expect, it } from "vitest";
import { getIcon } from "@iconify/vue";
import { agentBrandIcon } from "./agent-icons";

/** 已内嵌品牌图标的部署目标;其余目标使用通用 Bot 图标。 */
const TARGET_IDS = ["claude", "cursor", "copilot", "gemini", "codex", "opencode", "zcode"];

describe("agentBrandIcon", () => {
  it("每个部署目标都有已注册的品牌图标", () => {
    for (const id of TARGET_IDS) {
      const name = agentBrandIcon(id);
      expect(name).toBe(`agent-brand:${id}`);
      expect(getIcon(name!)).toBeTruthy();
    }
  });

  it("新增目标未内嵌品牌图标时回退通用图标", () => {
    for (const id of ["kimi", "dsh", "minimax", "pi"]) {
      expect(agentBrandIcon(id)).toBeNull();
    }
  });

  it("未知 id 返回 null,由调用方回退通用图标", () => {
    expect(agentBrandIcon("unknown-agent")).toBeNull();
  });
});
