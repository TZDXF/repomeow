/** 将当前语言的设置词条汇总为分类搜索文本，不读取用户配置。 */
export function settingsSearchText(value: unknown): string {
  if (typeof value === "string") return value;
  if (!value || typeof value !== "object") return "";
  return Object.values(value).map(settingsSearchText).join(" ");
}

export function matchesSettingsSearch(query: string, text: string): boolean {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  const normalized = text.toLocaleLowerCase();
  return terms.length > 0 && terms.every((term) => normalized.includes(term));
}

export const resourceSettingsTabs = ["skills", "mcp", "agents", "backup", "market"] as const;
export type ResourceSettingsTab = (typeof resourceSettingsTabs)[number];

export const resourceSettingsSearchEntries = resourceSettingsTabs.map((tab) => ({
  category: "resources",
  labelKey: `settings.resources.tabs.${tab}`,
  tab,
  searchKey: `settings.resources.${tab}`,
}));

export function resourceTabForSetting(labelKey: string): ResourceSettingsTab | undefined {
  return resourceSettingsSearchEntries.find((entry) => entry.labelKey === labelKey)?.tab;
}
