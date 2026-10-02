import type { Project } from "@/types";

/**
 * 报告历史项目筛选:关键词同时匹配名称与路径。
 * 语义与原 ReportHistory 内联实现一致,抽出以便复用与单测。
 */
export function filterProjectsByKeyword(projects: Project[], keyword: string): Project[] {
  const kw = keyword.trim().toLowerCase();
  if (!kw) return projects;
  return projects.filter(
    (p) => p.name.toLowerCase().includes(kw) || p.path.toLowerCase().includes(kw),
  );
}
