import type { Project } from "@/types";

/** 跟踪设置表格搜索:空格分词 AND,单词可命中名称、简介、路径或标签。 */
export function matchesTrackingProject(project: Project, query: string): boolean {
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) {
    return true;
  }
  const fields = [
    project.name,
    project.description,
    project.path,
    ...project.tags.map((tag) => tag.name),
  ].map((value) => value.toLowerCase());
  return terms.every((term) => fields.some((field) => field.includes(term)));
}
