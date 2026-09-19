/** 将当前语言的设置词条汇总为分类搜索文本，不读取用户配置。 */
export function settingsSearchText(value: unknown): string {
  if (typeof value === "string") return value;
  if (!value || typeof value !== "object") return "";
  return Object.values(value).map(settingsSearchText).join(" ");
}

function searchTerms(query: string): string[] {
  return query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
}

export function matchesSettingsSearch(query: string, text: string): boolean {
  const terms = searchTerms(query);
  const normalized = text.toLocaleLowerCase();
  return terms.length > 0 && terms.every((term) => normalized.includes(term));
}

export interface SettingsSearchFields {
  /** 设置项标题(权重最高) */
  title: string;
  /** 所属分类标题 */
  category: string;
  /** 原始 i18n key，兼容按英文 key 检索 */
  key?: string;
  /** 分类下全部文案(权重最低，命中时提取摘要解释命中原因) */
  details?: string;
}

export interface SettingsSearchScore {
  score: number;
  /** details 命中时截取的上下文摘要；标题已命中时为空 */
  snippet: string;
}

const FIELD_WEIGHTS = { title: 100, category: 20, key: 10, details: 1 } as const;
const SNIPPET_RADIUS = 24;

/** 在文本中定位首个命中词并截取上下文摘要，未命中返回空串。 */
export function searchSnippet(text: string, query: string): string {
  const terms = searchTerms(query);
  if (!terms.length) return "";
  const compact = text.replace(/\s+/g, " ").trim();
  const normalized = compact.toLocaleLowerCase();
  let first = -1;
  let termLength = 0;
  for (const term of terms) {
    const index = normalized.indexOf(term);
    if (index >= 0 && (first < 0 || index < first)) {
      first = index;
      termLength = term.length;
    }
  }
  if (first < 0) return "";
  const start = Math.max(0, first - SNIPPET_RADIUS);
  const end = Math.min(compact.length, first + termLength + SNIPPET_RADIUS);
  return `${start > 0 ? "…" : ""}${compact.slice(start, end)}${end < compact.length ? "…" : ""}`;
}

/**
 * 按字段权重为设置项打分：标题 > 分类 > i18n key > 全文案。
 * 每个查询词取其所中字段的最高权重累加，任一词未命中返回 null。
 */
export function scoreSettingsSearch(
  query: string,
  fields: SettingsSearchFields,
): SettingsSearchScore | null {
  const terms = searchTerms(query);
  if (!terms.length) return null;
  const normalized = {
    title: fields.title.toLocaleLowerCase(),
    category: fields.category.toLocaleLowerCase(),
    key: (fields.key ?? "").toLocaleLowerCase(),
    details: (fields.details ?? "").toLocaleLowerCase(),
  };
  let score = 0;
  let detailOnly = false;
  for (const term of terms) {
    let termScore = 0;
    if (normalized.title.includes(term)) termScore = FIELD_WEIGHTS.title;
    else if (normalized.category.includes(term)) termScore = FIELD_WEIGHTS.category;
    else if (normalized.key.includes(term)) termScore = FIELD_WEIGHTS.key;
    else if (normalized.details.includes(term)) {
      termScore = FIELD_WEIGHTS.details;
      detailOnly = true;
    }
    if (!termScore) return null;
    score += termScore;
  }
  return {
    score,
    snippet: detailOnly ? searchSnippet(fields.details ?? "", query) : "",
  };
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
