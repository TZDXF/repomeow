import type { Project } from "@/types";

export type TagFilterMode = "all" | "any";

interface ReportProjectFilterOptions {
  tagIds: number[];
  tagFilterMode?: TagFilterMode;
}

/** 报告项目选择器:关键词与标签条件组合;多标签默认交集,指定 any 时使用并集。 */
export function filterReportProjects(
  projects: Project[],
  keyword: string,
  { tagIds, tagFilterMode = "all" }: ReportProjectFilterOptions,
): Project[] {
  const query = keyword.trim().toLowerCase();
  return projects.filter((project) => {
    const matchesQuery =
      !query ||
      project.name.toLowerCase().includes(query) ||
      project.path.toLowerCase().includes(query);
    const matchesTags =
      !tagIds.length ||
      (tagFilterMode === "any"
        ? tagIds.some((id) => project.tags.some((tag) => tag.id === id))
        : tagIds.every((id) => project.tags.some((tag) => tag.id === id)));
    return matchesQuery && matchesTags;
  });
}

/** 全选当前可见项目,同时保留已选但被筛选隐藏的项目;项目 id 不重复。 */
export function selectVisibleProjectIds(
  selectedIds: number[],
  visibleProjects: Project[],
): number[] {
  return [...new Set([...selectedIds, ...visibleProjects.map((project) => project.id)])];
}
