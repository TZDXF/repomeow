import type { ReportHistoryDetail, ReportPeriodType } from "@/types";

export interface ReportExportLabels {
  collection: string;
  daily: string;
  weekly: string;
  dateRange: string;
  projects: string;
  generatedAt: string;
  commits: string;
  commitDetails: string;
}

interface ReportExportOptions {
  labels: ReportExportLabels;
  formatCreatedAt: (timestamp: number) => string;
}

function oneLine(value: string): string {
  return value.replace(/[\r\n]+/g, " ").trim();
}

function reportTypeLabel(type: ReportPeriodType, labels: ReportExportLabels): string {
  return type === "weekly" ? labels.weekly : labels.daily;
}

function reportDateLabel(report: ReportHistoryDetail): string {
  return report.dateFrom === report.dateTo
    ? report.dateFrom
    : `${report.dateFrom} – ${report.dateTo}`;
}

function reportHeading(report: ReportHistoryDetail, labels: ReportExportLabels): string {
  return `${reportTypeLabel(report.periodType, labels)} · ${reportDateLabel(report)}`;
}

function reportSection(
  report: ReportHistoryDetail,
  options: ReportExportOptions,
  headingLevel: 1 | 2,
): string {
  const { labels, formatCreatedAt } = options;
  const heading = "#".repeat(headingLevel);
  const lines = [
    `${heading} ${reportHeading(report, labels)}`,
    "",
    `- **${labels.dateRange}**: ${reportDateLabel(report)}`,
    `- **${labels.projects}**: ${report.projectNames.map(oneLine).join(", ")}`,
    `- **${labels.generatedAt}**: ${formatCreatedAt(report.createdAt)}`,
    `- **${labels.commits}**: ${report.totalCommits}`,
    "",
    report.result.trim(),
  ];

  const commits = report.commits.filter((item) => item.commits.length > 0);
  if (commits.length > 0) {
    lines.push("", `${heading}# ${labels.commitDetails}`);
    for (const item of commits) {
      lines.push("", `${heading}## ${oneLine(item.projectName)}`);
      for (const commit of item.commits) {
        const details = [oneLine(commit.author), oneLine(commit.date)].filter(Boolean).join(" · ");
        lines.push(
          `- \`${oneLine(commit.hash)}\` ${oneLine(commit.subject)}${details ? ` — ${details}` : ""}`,
        );
      }
    }
  }

  return lines.join("\n").trim();
}

/** 把一条或当前筛选范围内的多条日报/周报整理成可独立阅读的 Markdown 文档。 */
export function buildReportExportMarkdown(
  reports: ReportHistoryDetail[],
  options: ReportExportOptions,
): string {
  if (reports.length === 0) {
    return "";
  }
  if (reports.length === 1) {
    return `${reportSection(reports[0], options, 1)}\n`;
  }

  const from = reports.reduce(
    (minimum, report) => (report.dateFrom < minimum ? report.dateFrom : minimum),
    reports[0].dateFrom,
  );
  const to = reports.reduce(
    (maximum, report) => (report.dateTo > maximum ? report.dateTo : maximum),
    reports[0].dateTo,
  );
  const sections = reports.map((report) => reportSection(report, options, 2));
  return `# ${options.labels.collection} · ${from} – ${to}\n\n${sections.join("\n\n---\n\n")}\n`;
}

function safeFilenamePart(value: string): string {
  const invalidCharacters = '<>:"/\\|?*';
  return [...value]
    .map((character) =>
      character.charCodeAt(0) < 32 || invalidCharacters.includes(character) ? "-" : character,
    )
    .join("")
    .replace(/[. ]+$/g, "")
    .trim();
}

/** 根据导出内容生成 Windows/macOS 均可用的默认文件名。 */
export function createReportExportFilename(
  reports: ReportHistoryDetail[],
  labels: Pick<ReportExportLabels, "collection" | "daily" | "weekly">,
): string {
  if (reports.length === 0) {
    return `${safeFilenamePart(labels.collection)}.md`;
  }

  const from = reports.reduce(
    (minimum, report) => (report.dateFrom < minimum ? report.dateFrom : minimum),
    reports[0].dateFrom,
  );
  const to = reports.reduce(
    (maximum, report) => (report.dateTo > maximum ? report.dateTo : maximum),
    reports[0].dateTo,
  );
  const types = new Set(reports.map((report) => report.periodType));
  let name = labels.collection;
  if (types.size === 1) {
    name = reports[0].periodType === "weekly" ? labels.weekly : labels.daily;
  }
  const date = from === to ? from : `${from}_${to}`;
  return `${safeFilenamePart(name)}-${date}.md`;
}
