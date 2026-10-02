import type { ComposerTranslation } from "vue-i18n";
import type { ResourceSkillScanFinding, ResourceSkillScanReport } from "@/lib/resource-library";

/**
 * 技能安全扫描报告的展示辅助(预览页侧栏徽标与扫描结果面板共用):
 * 严重度/等级配色,以及各类标签的 i18n 解析(词条缺失时回落原始值)。
 */

export interface ScanLabelI18n {
  t: ComposerTranslation;
  te: (key: string) => boolean;
}

const SEVERITY_CLASSES: Record<string, string> = {
  critical: "border-red-600/30 bg-red-600/10 text-red-600 dark:text-red-400",
  high: "border-orange-500/30 bg-orange-500/10 text-orange-600 dark:text-orange-400",
  medium: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  low: "border-border bg-muted text-muted-foreground",
};

const LEVEL_CLASSES: Record<string, string> = {
  low: "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
  medium: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  high: "border-orange-500/30 bg-orange-500/10 text-orange-600 dark:text-orange-400",
  critical: "border-red-600/30 bg-red-600/10 text-red-600 dark:text-red-400",
};

export function scanSeverityClass(severity: string): string {
  return SEVERITY_CLASSES[severity] ?? SEVERITY_CLASSES.medium;
}

export function scanLevelClass(level: string): string {
  return LEVEL_CLASSES[level] ?? LEVEL_CLASSES.medium;
}

export function scanLevelLabel(i18n: ScanLabelI18n, level: string): string {
  const key = `settings.resources.skills.previewPage.scan.level.${level}`;
  return i18n.te(key) ? i18n.t(key) : level;
}

export function scanSeverityLabel(i18n: ScanLabelI18n, severity: string): string {
  const key = `settings.resources.skills.previewPage.scan.severity.${severity}`;
  return i18n.te(key) ? i18n.t(key) : severity;
}

export function scanCategoryLabel(i18n: ScanLabelI18n, finding: ResourceSkillScanFinding): string {
  const key = `settings.resources.skills.previewPage.scan.category.${finding.category}`;
  return i18n.te(key) ? i18n.t(key) : finding.category;
}

/** 静态发现标题走规则 i18n;AI 发现直接用模型输出标题 */
export function scanFindingTitle(i18n: ScanLabelI18n, finding: ResourceSkillScanFinding): string {
  if (finding.ruleId) {
    const key = `settings.resources.skills.previewPage.scan.rules.${finding.ruleId}`;
    if (i18n.te(key)) return i18n.t(key);
  }
  return finding.title || finding.category;
}

/** AI 语义层异常提示(skipped/canceled/failed);正常或无报告时为空串 */
export function scanLlmNotice(i18n: ScanLabelI18n, report: ResourceSkillScanReport | null): string {
  if (!report) return "";
  if (report.llmStatus === "skipped") {
    return i18n.t("settings.resources.skills.previewPage.scan.llmSkipped");
  }
  if (report.llmStatus === "canceled") {
    return i18n.t("settings.resources.skills.previewPage.scan.llmCanceled");
  }
  if (report.llmStatus === "failed") {
    return i18n.t("settings.resources.skills.previewPage.scan.llmFailed", {
      error: report.llmErrorMessage || report.llmErrorCode || "",
    });
  }
  return "";
}
