import { cmd } from "@/lib/tauri";
import type { ProjectSkill } from "@/types";

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
export type RepairAction = "reapply" | "detach";
export interface ResourceRepairInput {
  path: string;
  kind: ProjectResourceKind;
  resourceId: string;
  agentId: string;
  /** reapply: 覆盖更新——以来源最新定义重写项目内容,丢弃本地修改;detach: 强制解除——仅删除托管记录,保留项目文件。 */
  action: RepairAction;
  expectedRevision: string;
}
/** 修复异常部署(modified/conflict 等)的兜底出口。 */
export const repairProjectResource = (input: ResourceRepairInput) =>
  cmd<void>("project_ai_repair", { ...input });

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

/** 同名技能跨 Agent 目录去重后的合并视图:一个技能一行,dirs 记录全部实例目录。 */
export interface MergedSkill {
  name: string;
  description: string;
  /** 全部同名技能目录(仓库相对路径),按 Agent 目标顺序稳定排列,首项为主来源。 */
  dirs: string[];
}

/**
 * 非托管技能按名称去重:同一技能散落在多个 Agent skills 目录时合并为一行。
 * 扫描层保留全部实例(与 Agent 显隐设置无关),合并只发生在展示层;
 * skillPaths 为各 Agent 的技能根(按 TARGETS 顺序),目录按所属根排序,
 * 首项作为导入/认领/预览的主来源,描述取首个非空值。
 */
export function mergeSkillsByName(skills: ProjectSkill[], skillPaths: string[]): MergedSkill[] {
  const rank = (dir: string) => {
    const index = skillPaths.findIndex((root) => dir.startsWith(`${root}/`));
    return index === -1 ? skillPaths.length : index;
  };
  const byName = new Map<string, ProjectSkill[]>();
  for (const skill of skills) {
    const group = byName.get(skill.name);
    if (group) {
      group.push(skill);
    } else {
      byName.set(skill.name, [skill]);
    }
  }
  return [...byName.values()].map((group) => {
    const ordered = [...group].sort(
      (a, b) => rank(a.dir) - rank(b.dir) || a.dir.localeCompare(b.dir),
    );
    return {
      name: ordered[0].name,
      // 描述跟随主来源(排序后首个目录),为空时取其他实例的首个非空值。
      description: ordered[0].description || ordered.find((s) => s.description)?.description || "",
      dirs: ordered.map((s) => s.dir),
    };
  });
}
