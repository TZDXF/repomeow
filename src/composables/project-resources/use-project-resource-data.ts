import { computed, onBeforeUnmount, ref, watch, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import {
  loadProjectResources,
  mergeSkillsByName,
  resourceTree,
  type ProjectAiTarget,
  type ProjectResourceKind,
  type ProjectResourceSnapshot,
  type ResourceChoice,
  type ResourceDeployment,
  type ResourceTreeGroup,
} from "@/lib/project-ai-resources";
import { joinPath } from "@/lib/path";
import type { ProjectAiAssets } from "@/types";

export interface UnmanagedItem {
  key: string;
  name: string;
  /** 技能描述(仅 skills 有;mcp 为空串)。 */
  description: string;
  /** 预览路径(skills 为主来源的 SKILL.md 路径,mcp 为配置文件路径)。 */
  path: string;
  /** 主来源:skills = 首个技能目录;mcp = 配置文件路径。 */
  source: string;
  /** 全部来源:skills 同名去重后的所有技能目录(含主来源);mcp 恒为单元素。 */
  sources: string[];
}

/** 分组筛选里的「未分组」桶 id(真实分组不会用到)。 */
export const UNGROUPED_FILTER = "__ungrouped";

/** 项目资源列表 = 已添加(shortlist)∪ 已部署;来源被删除时回退部署记录里的名字。 */
export function buildListedResources(data: ProjectResourceSnapshot | null): ResourceChoice[] {
  const ids = new Set<string>(data?.shortlist ?? []);
  for (const d of data?.deployments ?? []) {
    ids.add(d.resourceId);
  }
  const resources: ResourceChoice[] = [];
  for (const id of ids) {
    const existing = data?.resources.find((r) => r.id === id);
    if (existing) {
      resources.push(existing);
      continue;
    }
    const record = data?.deployments.find((d) => d.resourceId === id);
    resources.push({
      id,
      name: record?.name ?? id,
      description: "",
      groupIds: [],
      supportedAgents: [],
    });
  }
  return resources;
}

/** 非托管条目:skills 按名称跨 Agent 目录去重(扫描保留全部实例,合并只在展示层)。 */
export function buildUnmanagedItems(
  kind: ProjectResourceKind,
  assets: ProjectAiAssets | null,
  deployments: ResourceDeployment[],
  targets: ProjectAiTarget[],
): UnmanagedItem[] {
  if (kind === "skills") {
    const skills = assets?.skills.filter((s) => !deployments.some((d) => d.path === s.dir)) ?? [];
    return mergeSkillsByName(
      skills,
      targets.map((a) => a.skillPath),
    ).map((s) => ({
      key: `skill:${s.name}`,
      name: s.name,
      path: joinPath(s.dirs[0], "SKILL.md"),
      source: s.dirs[0],
      sources: s.dirs,
      description: s.description,
    }));
  }
  const servers =
    assets?.mcp.flatMap((file) =>
      file.servers
        .filter((s) => !deployments.some((d) => d.path === file.path && d.name === s.name))
        .map((s) => ({ name: s.name, path: file.path })),
    ) ?? [];
  return servers.map((s) => ({
    key: `mcp:${s.path}:${s.name}`,
    name: s.name,
    path: s.path,
    source: s.path,
    sources: [s.path],
    description: "",
  }));
}

export function findDeployment(
  records: ResourceDeployment[],
  agentId: string,
): ResourceDeployment | undefined {
  return records.find((d) => d.agentId === agentId);
}

export function resourceSupported(resource: ResourceChoice, agent: ProjectAiTarget): boolean {
  return resource.supportedAgents.includes(agent.id);
}

/** 可切换部署:受支持,或虽不支持但已部署(允许解除配置)。 */
export function resourceSelectable(
  resource: ResourceChoice,
  agent: ProjectAiTarget,
  records: ResourceDeployment[],
): boolean {
  return resourceSupported(resource, agent) || !!findDeployment(records, agent.id);
}

/** 可见 Agent:设置页未隐藏的目标 + 虽隐藏但已部署该资源的 Agent(允许解除配置)。 */
export function visibleResourceAgents(
  targets: ProjectAiTarget[],
  kind: ProjectResourceKind,
  records: ResourceDeployment[],
  hiddenAgents: string[],
): ProjectAiTarget[] {
  return targets.filter(
    (a) =>
      ((kind === "skills" || !!a.mcpPath) && !hiddenAgents.includes(a.id)) ||
      findDeployment(records, a.id),
  );
}

/**
 * 已认领的本地来源(配置到其他 Agent 后进入托管列表的 local: 资源)仍可随时收入资源库:
 * 后端 import 会经 migrate_local 把部署记录改挂到库资源 ID,本地来源记录随之清除。
 * 来源与名称优先取部署记录;无部署记录时从 local id 解析(local:skills:<dir> / local:mcp:<path>#<name>)。
 */
export function localImportTargetOf(
  resource: ResourceChoice,
  records: ResourceDeployment[],
  kind: ProjectResourceKind,
): { source: string; name?: string } | null {
  if (!resource.id.startsWith("local:")) {
    return null;
  }
  const rest = resource.id.slice(`local:${kind}:`.length);
  if (kind === "skills") {
    // 同名多副本时以主来源(id 内嵌的认领目录)为准,而非首个部署记录。
    return rest ? { source: rest } : null;
  }
  const record = records[0];
  if (record) {
    return { source: record.path, name: record.name };
  }
  const hash = rest.lastIndexOf("#");
  return hash > 0 ? { source: rest.slice(0, hash), name: rest.slice(hash + 1) } : null;
}

/** 非托管行的归属 Agent 集合:skills 同名合并后可有多个归属(每个目录前缀各算一个)。 */
export function unmanagedOwners(
  item: UnmanagedItem,
  targets: ProjectAiTarget[],
  kind: ProjectResourceKind,
): string[] {
  if (kind === "skills") {
    return item.sources
      .map((dir) => targets.find((a) => dir.startsWith(`${a.skillPath}/`))?.id)
      .filter((id): id is string => !!id);
  }
  const owner = targets.find((a) => a.mcpPath === item.source);
  return owner ? [owner.id] : [];
}

/** 该 Agent 名下的实例来源(skills 为其目录,mcp 为配置文件路径);非归属返回空。 */
export function unmanagedOwnerSource(
  item: UnmanagedItem,
  agentId: string,
  targets: ProjectAiTarget[],
  kind: ProjectResourceKind,
): string {
  const target = targets.find((a) => a.id === agentId);
  if (!target) {
    return "";
  }
  if (kind === "skills") {
    return item.sources.find((dir) => dir.startsWith(`${target.skillPath}/`)) ?? "";
  }
  return target.mcpPath === item.source ? item.source : "";
}

/** 非托管行可见 Agent:跟随设置显隐(扫描与显隐无关,只在展示层过滤,全行表现一致)。 */
export function visibleUnmanagedAgents(
  targets: ProjectAiTarget[],
  kind: ProjectResourceKind,
  hiddenAgents: string[],
): ProjectAiTarget[] {
  return targets.filter((a) => (kind === "skills" || !!a.mcpPath) && !hiddenAgents.includes(a.id));
}

export interface ProjectResourceDataOptions {
  projectPath: () => string;
  kind: () => ProjectResourceKind;
  revision: () => number;
  targets: () => ProjectAiTarget[];
  assets: () => ProjectAiAssets | null;
}

/**
 * 项目资源区数据层:快照加载(序号守卫防过期提交)、托管/非托管列表派生、
 * 分组筛选与部署记录查询。动作(分配/导入/删除/修复)见 use-resource-actions /
 * use-resource-repair;变更后不自行刷新,由 changed 回调驱动父组件重扫资产。
 */
export function useProjectResourceData(options: ProjectResourceDataOptions) {
  const { t } = useI18n();
  const data = ref<ProjectResourceSnapshot | null>(null);
  const loading = ref(false);
  const error = ref("");
  const activeFilter = ref("");
  /** 非托管检测用的 assets 快照:仅在 load() 提交 deployments 时同步更新,保证两侧数据成对一致,杜绝刷新期间的幽灵行。 */
  const assetsSnapshot = ref<ProjectAiAssets | null>(null);
  let sequence = 0;
  onBeforeUnmount(() => sequence++);
  /** 已有数据时的刷新走静默模式(不闪 loading),配合父组件协调好的时序,避免整列视觉闪动。 */
  async function load() {
    const seq = ++sequence;
    const silent = !!data.value;
    if (!silent) {
      loading.value = true;
    }
    error.value = "";
    try {
      const next = await loadProjectResources(options.projectPath(), options.kind());
      if (seq === sequence) {
        data.value = next;
        assetsSnapshot.value = options.assets();
      }
    } catch (e) {
      if (seq === sequence) {
        error.value = String(e);
        data.value = null;
      }
    } finally {
      if (!silent && seq === sequence) {
        loading.value = false;
      }
    }
  }
  watch(
    () => [options.projectPath(), options.kind(), options.revision()],
    () => {
      void load();
    },
    { immediate: true },
  );
  watch(
    () => [options.projectPath(), options.kind()],
    () => {
      data.value = null;
      assetsSnapshot.value = null;
      activeFilter.value = "";
    },
  );
  const listed = computed(() => buildListedResources(data.value));
  const unmanaged = computed<UnmanagedItem[]>(() =>
    buildUnmanagedItems(
      options.kind(),
      assetsSnapshot.value,
      data.value?.deployments ?? [],
      options.targets(),
    ),
  );
  /** 分组只作为筛选维度(skills):用户分组与市场来源(owner/repo)并列。 */
  const tree = computed(() =>
    resourceTree(
      options.kind() === "skills" ? (data.value?.groups ?? []) : [],
      listed.value,
      t("projectAi.ungrouped"),
    ).filter((g) => g.resources.length),
  );
  /** 未入库条目不单独区分:无未分组桶时补一个,使其计入筛选。 */
  const filters = computed(() => {
    if (options.kind() !== "skills") {
      return [];
    }
    const groups = tree.value;
    if (unmanaged.value.length && !groups.some((g) => g.id === UNGROUPED_FILTER)) {
      return [...groups, { id: UNGROUPED_FILTER, name: t("projectAi.ungrouped"), resources: [] }];
    }
    return groups;
  });
  const filtered = computed(() => {
    if (!activeFilter.value) {
      return listed.value;
    }
    return tree.value.find((g) => g.id === activeFilter.value)?.resources ?? [];
  });
  watch(filters, (next) => {
    if (activeFilter.value && !next.some((g) => g.id === activeFilter.value)) {
      activeFilter.value = "";
    }
  });
  /** 当前筛选下可见的未入库条目:全部与未分组筛选均展示。 */
  const filteredUnmanaged = computed(() =>
    !activeFilter.value || activeFilter.value === UNGROUPED_FILTER ? unmanaged.value : [],
  );
  /** 分组计数:未分组桶并入未入库条目数。 */
  function groupCount(group: ResourceTreeGroup) {
    return group.id === UNGROUPED_FILTER
      ? group.resources.length + unmanaged.value.length
      : group.resources.length;
  }
  function records(id: string): ResourceDeployment[] {
    return data.value?.deployments.filter((d) => d.resourceId === id) ?? [];
  }
  return {
    data: data as Ref<ProjectResourceSnapshot | null>,
    loading,
    error,
    assetsSnapshot,
    activeFilter,
    listed,
    unmanaged,
    filters,
    filtered,
    filteredUnmanaged,
    groupCount,
    records,
  };
}
