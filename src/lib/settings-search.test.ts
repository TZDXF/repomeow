import { describe, expect, it } from "vitest";
import {
  resourceSettingsSearchEntries,
  resourceTabForSetting,
  matchesSettingsSearch,
  scoreSettingsSearch,
  searchSnippet,
  settingsSearchText,
} from "./settings-search";

describe("settings search", () => {
  it("matches Chinese, case-insensitive English and multiple keywords", () => {
    expect(matchesSettingsSearch("主题", "常规 主题")).toBe(true);
    expect(matchesSettingsSearch("  AI  model ", "AI default Model")).toBe(true);
    expect(matchesSettingsSearch("AI missing", "AI model")).toBe(false);
    expect(matchesSettingsSearch("  ", "主题")).toBe(false);
  });
  it("includes nested descriptions and tolerates non-text values", () => {
    expect(settingsSearchText({ title: "资源库", child: { backup: "备份" }, other: null })).toBe(
      "资源库 备份 ",
    );
    expect(settingsSearchText(undefined)).toBe("");
  });
});

describe("resource settings navigation", () => {
  it.each(["skills", "mcp", "agents", "backup", "market"])(
    "indexes and resolves the %s tab",
    (tab) => {
      const entry = resourceSettingsSearchEntries.find((item) => item.tab === tab)!;
      expect(entry.category).toBe("resources");
      expect(matchesSettingsSearch(tab.toUpperCase(), entry.labelKey)).toBe(true);
      expect(resourceTabForSetting(entry.labelKey)).toBe(tab);
    },
  );
  it("does not select a tab for other settings", () => {
    expect(resourceTabForSetting("settings.categories.resources")).toBeUndefined();
    expect(resourceTabForSetting("settings.general.theme")).toBeUndefined();
  });
});

describe("settings search scoring", () => {
  it("ranks title matches above detail-only matches", () => {
    const titleHit = scoreSettingsSearch("用户", {
      title: "用户名",
      category: "账号绑定",
      details: "保存时验证 Token",
    })!;
    const detailHit = scoreSettingsSearch("用户", {
      title: "CLI",
      category: "CLI",
      details: "将程序目录加入用户 PATH",
    })!;
    expect(titleHit.score).toBeGreaterThan(detailHit.score);
    expect(titleHit.snippet).toBe("");
    expect(detailHit.snippet).toContain("用户 PATH");
  });
  it("returns null when any term misses and handles blank query", () => {
    const fields = { title: "主题", category: "常规" };
    expect(scoreSettingsSearch("主题 缺失", fields)).toBeNull();
    expect(scoreSettingsSearch("  ", fields)).toBeNull();
  });
  it("extracts a snippet around the first matched term", () => {
    const text = "前缀文字 ".repeat(10) + "用户 PATH 已更新" + " 后缀文字".repeat(10);
    const snippet = searchSnippet(text, "用户");
    expect(snippet).toContain("用户 PATH");
    expect(snippet.startsWith("…")).toBe(true);
    expect(snippet.endsWith("…")).toBe(true);
  });
  it("returns empty snippet without a match", () => {
    expect(searchSnippet("常规 主题", "用户")).toBe("");
  });
});
