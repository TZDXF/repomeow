import { describe, expect, it } from "vitest";
import { buildReportExportMarkdown, createReportExportFilename } from "@/lib/report-export";
import type { ReportHistoryDetail } from "@/types";

const labels = {
  collection: "报告导出",
  daily: "日报",
  weekly: "周报",
  dateRange: "报告周期",
  projects: "项目",
  generatedAt: "生成时间",
  commits: "提交数",
  commitDetails: "提交记录",
};

function report(overrides: Partial<ReportHistoryDetail> = {}): ReportHistoryDetail {
  return {
    id: 1,
    projectIds: [1],
    dateFrom: "2026-08-29",
    dateTo: "2026-08-29",
    rangeLabel: "2026-08-29",
    authorMode: "all",
    language: "zh-CN",
    periodType: "daily",
    createdAt: 123,
    projectNames: ["RepoMeow"],
    totalCommits: 1,
    result: "今日总结\n\n- 完成导出",
    commits: [
      {
        projectId: 1,
        projectName: "RepoMeow",
        projectDescription: "",
        commits: [
          {
            hash: "abc1234",
            author: "Alice",
            date: "2026-08-29 10:00",
            subject: "feat: 增加导出",
          },
        ],
      },
    ],
    ...overrides,
  };
}

const options = { labels, formatCreatedAt: (timestamp: number) => `time-${timestamp}` };

describe("buildReportExportMarkdown", () => {
  it("导出单条日报时包含元数据、正文和提交明细", () => {
    const content = buildReportExportMarkdown([report()], options);

    expect(content).toContain("# 日报 · 2026-08-29");
    expect(content).toContain("**项目**: RepoMeow");
    expect(content).toContain("今日总结\n\n- 完成导出");
    expect(content).toContain("## 提交记录");
    expect(content).toContain("`abc1234` feat: 增加导出 — Alice · 2026-08-29 10:00");
    expect(content.endsWith("\n")).toBe(true);
  });

  it("导出多条报告时生成总标题并分隔日报和周报", () => {
    const content = buildReportExportMarkdown(
      [
        report(),
        report({
          id: 2,
          periodType: "weekly",
          dateFrom: "2026-08-24",
          dateTo: "2026-08-28",
          result: "本周总结",
          commits: [],
        }),
      ],
      options,
    );

    expect(content).toContain("# 报告导出 · 2026-08-24 – 2026-08-29");
    expect(content).toContain("## 日报 · 2026-08-29");
    expect(content).toContain("## 周报 · 2026-08-24 – 2026-08-28");
    expect(content).toContain("\n\n---\n\n");
  });

  it("空列表返回空内容", () => {
    expect(buildReportExportMarkdown([], options)).toBe("");
  });
});

describe("createReportExportFilename", () => {
  it("单条周报使用周报与日期范围命名", () => {
    expect(
      createReportExportFilename(
        [report({ periodType: "weekly", dateFrom: "2026-08-24", dateTo: "2026-08-28" })],
        labels,
      ),
    ).toBe("周报-2026-08-24_2026-08-28.md");
  });

  it("混合类型使用报告导出命名", () => {
    expect(createReportExportFilename([report(), report({ periodType: "weekly" })], labels)).toBe(
      "报告导出-2026-08-29.md",
    );
  });
});
