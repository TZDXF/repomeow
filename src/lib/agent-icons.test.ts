import { describe, expect, it } from "vitest";
import { getIcon } from "@iconify/vue";
import { agentBrandIcon } from "./agent-icons";

/** 与 Rust 侧 commands/ai/assets/deployment.rs 的 TARGETS id 对齐。 */
const TARGET_IDS = ["claude", "cursor", "copilot", "gemini", "codex", "opencode", "zcode"];

describe("agentBrandIcon", () => {
  it("每个部署目标都有已注册的品牌图标", () => {
    for (const id of TARGET_IDS) {
      const name = agentBrandIcon(id);
      expect(name).toBe(`agent-brand:${id}`);
      expect(getIcon(name!)).toBeTruthy();
    }
  });

  it("未知 id 返回 null,由调用方回退通用图标", () => {
    expect(agentBrandIcon("unknown-agent")).toBeNull();
  });
});
