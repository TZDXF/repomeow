import { describe, expect, it } from "vitest";
import {
  resourceSettingsSearchEntries,
  resourceTabForSetting,
  matchesSettingsSearch,
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
