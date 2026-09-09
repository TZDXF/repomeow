import { cmd } from "@/lib/tauri";

export type ProjectResourceKind = "skills" | "mcp";
export interface ProjectAiTarget {
  id: string;
  name: string;
  skillPath: string;
  mcpPath: string;
}
export interface ResourceChoice {
  id: string;
  name: string;
  description: string;
  groupIds: string[];
  supportedAgents: string[];
  /** 市场技能来源(owner/repo);手动创建技能与 MCP 无此字段。 */
  source?: string;
}
export interface ResourceGroup {
  id: string;
  name: string;
  description: string;
  color?: string;
}
export type DeploymentState =
  | "configured"
  | "update"
  | "modified"
  | "missing"
  | "sourceMissing"
  | "sourceUnavailable"
  | "conflict";
export interface ResourceDeployment {
  kind: ProjectResourceKind;
  agentId: string;
  resourceId: string;
  name: string;
  path: string;
  fingerprint: string;
  status: DeploymentState;
}
export interface ProjectResourceSnapshot {
  revision: string;
  groups: ResourceGroup[];
  resources: ResourceChoice[];
  deployments: ResourceDeployment[];
  /** 已加入项目列表的资源 ID(含尚未配置任何 Agent 的项)。 */
  shortlist: string[];
  sourceError: string | null;
}
export interface ResourceApplyResult {
  applied: number;
  failures: { resourceId: string; code: string; message: string }[];
}
export interface ResourceAssignResult {
  applied: number;
  failures: { agentId: string; code: string; message: string }[];
}
export const listProjectAiTargets = () => cmd<ProjectAiTarget[]>("project_ai_targets");
export const loadProjectResources = (path: string, kind: ProjectResourceKind) =>
  cmd<ProjectResourceSnapshot>("project_ai_resources", { path, kind });
export interface ResourceAddInput {
  path: string;
  kind: ProjectResourceKind;
  resourceIds: string[];
  expectedRevision: string;
}
export const addProjectResources = (input: ResourceAddInput) =>
  cmd<ResourceApplyResult>("project_ai_add", { ...input });
export interface ResourceAssignInput {
  path: string;
  kind: ProjectResourceKind;
  resourceId: string;
  agentIds: string[];
  expectedRevision: string;
}
export const assignProjectResource = (input: ResourceAssignInput) =>
  cmd<ResourceAssignResult>("project_ai_assign", { ...input });
export interface ResourceRemoveInput {
  path: string;
  kind: ProjectResourceKind;
  resourceId: string;
  expectedRevision: string;
}
export const removeProjectResource = (input: ResourceRemoveInput) =>
  cmd<ResourceAssignResult>("project_ai_remove", { ...input });
export interface ResourceImportInput {
  path: string;
  kind: ProjectResourceKind;
  /** skills: 项目内技能目录(如 .claude/skills/foo);mcp: 配置文件路径。 */
  source: string;
  /** mcp 专用:服务器名。 */
  name?: string;
  expectedRevision: string;
}
export interface ResourceImportResult {
  resourceId: string;
  /** 资源库已存在同名条目(直接认领,未新建)。 */
  existed: boolean;
}
export const importProjectResource = (input: ResourceImportInput) =>
  cmd<ResourceImportResult>("project_ai_import", { ...input });

export interface ResourceClaimInput {
  path: string;
  kind: ProjectResourceKind;
  /** skills: 项目内技能目录(如 .claude/skills/foo);mcp: 配置文件路径。 */
  source: string;
  /** mcp 专用:服务器名。 */
  name?: string;
  expectedRevision: string;
}
export interface ResourceClaimOutcome {
  resourceId: string;
}
/** 认领非托管资源为项目本地来源(记录来源、不进资源库),来源 Agent 按现状登记为已配置。 */
export const claimLocalProjectResource = (input: ResourceClaimInput) =>
  cmd<ResourceClaimOutcome>("project_ai_claim_local", { ...input });

/** 删除非托管资源:skills 删除整个技能目录,mcp 从配置文件中移除服务器条目;已托管路径后端拒绝。 */
export const deleteUnmanagedProjectResource = (input: ResourceClaimInput) =>
  cmd<void>("project_ai_delete_unmanaged", { ...input });

export interface ResourceTreeGroup {
  id: string;
  name: string;
  resources: ResourceChoice[];
}
/**
 * 多分组只影响展示,选择以稳定 ID 为单一事实源。空组保留给选择器展示。
 * 市场来源(owner/repo)视为附加分组:与用户分组并列,两个维度都命中的资源重复出现。
 */
export function resourceTree(
  groups: ResourceGroup[],
  resources: ResourceChoice[],
  ungrouped: string,
): ResourceTreeGroup[] {
  const groupIds = new Set(groups.map((g) => g.id));
  const tree = groups.map((g) => ({
    id: g.id,
    name: g.name,
    resources: resources.filter((r) => r.groupIds.includes(g.id)),
  }));
  const sources = [
    ...new Set(resources.map((r) => r.source).filter((s): s is string => !!s)),
  ].sort();
  for (const source of sources) {
    tree.push({
      id: `source:${source}`,
      name: source,
      resources: resources.filter((r) => r.source === source),
    });
  }
  const loose = resources.filter((r) => !r.groupIds.some((id) => groupIds.has(id)) && !r.source);
  if (loose.length) {
    tree.push({ id: "__ungrouped", name: ungrouped, resources: loose });
  }
  return tree;
}
export function selectionState(
  ids: string[],
  selected: ReadonlySet<string>,
): "none" | "some" | "all" {
  const unique = [...new Set(ids)];
  const count = unique.filter((id) => selected.has(id)).length;
  if (count === 0) {
    return "none";
  }
  return count === unique.length ? "all" : "some";
}
export function toggleResources(
  selected: ReadonlySet<string>,
  ids: string[],
  checked: boolean,
): Set<string> {
  const next = new Set(selected);
  for (const id of ids) {
    if (checked) {
      next.add(id);
    } else {
      next.delete(id);
    }
  }
  return next;
}
/** 单个资源的 Agent 勾选变化:新增/移除按目标数计,可更新与缺失计入更新。 */
export function assignChanges(
  deployments: ResourceDeployment[],
  resourceId: string,
  selectedAgents: ReadonlySet<string>,
) {
  const existing = deployments.filter((d) => d.resourceId === resourceId);
  const agents = new Set(existing.map((d) => d.agentId));
  return {
    add: [...selectedAgents].filter((a) => !agents.has(a)).length,
    remove: existing.filter((d) => !selectedAgents.has(d.agentId)).length,
    update: existing.filter(
      (d) => selectedAgents.has(d.agentId) && (d.status === "update" || d.status === "missing"),
    ).length,
  };
}
