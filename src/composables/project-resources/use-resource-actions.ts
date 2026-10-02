import { ref, watch, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  assignProjectResource,
  claimLocalProjectResource,
  deleteUnmanagedProjectResource,
  importProjectResource,
  loadProjectResources,
  removeProjectResource,
  type ProjectAiTarget,
  type ProjectResourceKind,
  type ProjectResourceSnapshot,
  type ResourceChoice,
  type ResourceDeployment,
} from "@/lib/project-ai-resources";
import {
  localImportTargetOf,
  resourceSelectable,
  unmanagedOwners,
  type UnmanagedItem,
} from "./use-project-resource-data";

export interface ResourceActionsOptions {
  projectPath: () => string;
  kind: () => ProjectResourceKind;
  targets: () => ProjectAiTarget[];
  data: Ref<ProjectResourceSnapshot | null>;
  records: (id: string) => ResourceDeployment[];
  /** 变更后不自行刷新:通知父组件先重扫 assets,再由 revision 驱动数据层静默刷新,避免托管/非托管数据错位导致的闪烁。 */
  changed: () => void;
}

/**
 * 项目资源动作层:Agent 分配切换、本地/非托管导入、托管移除与非托管删除。
 * 所有写操作携带 expectedRevision 做乐观并发控制;批量操作逐项重取快照拿最新
 * revision(上一步写入会使其失效);toggling/importing/removing 为防重入锁。
 */
export function useResourceActions(options: ResourceActionsOptions) {
  const { t } = useI18n();
  const removeTargets = ref<ResourceChoice[]>([]);
  const removing = ref(false);
  const unmanagedRemoveTarget = ref<UnmanagedItem | null>(null);
  const removingUnmanaged = ref(false);
  const importing = ref<string | null>(null);
  const toggling = ref("");
  // 项目/类别切换:重置动作层瞬态(数据层自行重置快照与筛选)
  watch(
    () => [options.projectPath(), options.kind()],
    () => {
      removeTargets.value = [];
      unmanagedRemoveTarget.value = null;
      toggling.value = "";
    },
  );

  /** 点击 Agent 标签即切换部署:新增勾选或解除该 Agent 的托管配置。 */
  async function toggleAgent(resource: ResourceChoice, agent: ProjectAiTarget) {
    if (
      !options.data.value ||
      toggling.value ||
      !resourceSelectable(resource, agent, options.records(resource.id))
    ) {
      return;
    }
    const current = new Set(options.records(resource.id).map((d) => d.agentId));
    if (current.has(agent.id)) {
      current.delete(agent.id);
    } else {
      current.add(agent.id);
    }
    toggling.value = `${resource.id}:${agent.id}`;
    try {
      const result = await assignProjectResource({
        path: options.projectPath(),
        kind: options.kind(),
        resourceId: resource.id,
        agentIds: [...current],
        expectedRevision: options.data.value.revision,
      });
      if (result.failures.length) {
        toast.error(
          t("projectAi.partial", { count: result.applied, failed: result.failures.length }),
        );
      }
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      toggling.value = "";
    }
  }

  /**
   * 非托管资源直接配置 Agent:先认领为项目本地来源(记录来源、不入库,
   * 来源 Agent 按现状登记),再把目标集合设为「现状 ∪ 点击的 Agent」一次 assign。
   * skills 同名合并行:其余归属 Agent 一并并入目标集合,同名副本统一纳入同一托管记录。
   */
  async function configureUnmanaged(item: UnmanagedItem, agent: ProjectAiTarget) {
    if (!options.data.value || toggling.value) {
      return;
    }
    toggling.value = `${item.key}:${agent.id}`;
    try {
      const outcome = await claimLocalProjectResource({
        path: options.projectPath(),
        kind: options.kind(),
        source: item.source,
        name: options.kind() === "mcp" ? item.name : undefined,
        expectedRevision: options.data.value.revision,
      });
      const snapshot = await loadProjectResources(options.projectPath(), options.kind());
      const current = new Set(
        snapshot.deployments
          .filter((d) => d.resourceId === outcome.resourceId)
          .map((d) => d.agentId),
      );
      for (const owner of unmanagedOwners(item, options.targets(), options.kind())) {
        current.add(owner);
      }
      current.add(agent.id);
      const result = await assignProjectResource({
        path: options.projectPath(),
        kind: options.kind(),
        resourceId: outcome.resourceId,
        agentIds: [...current],
        expectedRevision: snapshot.revision,
      });
      if (result.failures.length) {
        toast.error(
          t("projectAi.partial", { count: result.applied, failed: result.failures.length }),
        );
      } else {
        toast.success(t("projectAi.applied", { count: result.applied }));
      }
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      toggling.value = "";
    }
  }

  /** 已认领的本地来源收入资源库(migrate_local 改挂库资源 ID)。 */
  async function importLocal(resource: ResourceChoice) {
    const target = localImportTargetOf(resource, options.records(resource.id), options.kind());
    if (!options.data.value || importing.value || !target) {
      return;
    }
    importing.value = resource.id;
    try {
      await importProjectResource({
        path: options.projectPath(),
        kind: options.kind(),
        source: target.source,
        name: target.name,
        expectedRevision: options.data.value.revision,
      });
      toast.success(t("projectAi.imported", { name: resource.name }));
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      importing.value = null;
    }
  }

  async function importUnmanaged(item: UnmanagedItem) {
    if (!options.data.value || importing.value) {
      return;
    }
    importing.value = item.key;
    try {
      // skills 同名合并行:逐目录导入(同名复用同一库条目,各目录归属 Agent 按现状认领);
      // 每次导入会使 revision 失效,逐项重取快照。
      for (const source of item.sources) {
        const snapshot = await loadProjectResources(options.projectPath(), options.kind());
        await importProjectResource({
          path: options.projectPath(),
          kind: options.kind(),
          source,
          name: options.kind() === "mcp" ? item.name : undefined,
          expectedRevision: snapshot.revision,
        });
      }
      toast.success(t("projectAi.imported", { name: item.name }));
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      importing.value = null;
    }
  }

  /** 删除非托管资源:经后端校验未托管后直接清理磁盘文件/配置条目,成功后走父组件统一刷新。 */
  async function confirmRemoveUnmanaged() {
    const target = unmanagedRemoveTarget.value;
    if (!target || !options.data.value || removingUnmanaged.value) {
      return;
    }
    removingUnmanaged.value = true;
    try {
      // skills 同名合并行:逐目录删除;每次删除会使 revision 失效,逐项重取快照。
      for (const source of target.sources) {
        const snapshot = await loadProjectResources(options.projectPath(), options.kind());
        await deleteUnmanagedProjectResource({
          path: options.projectPath(),
          kind: options.kind(),
          source,
          name: options.kind() === "mcp" ? target.name : undefined,
          expectedRevision: snapshot.revision,
        });
      }
      toast.success(t("projectAi.removed", { name: target.name }));
      unmanagedRemoveTarget.value = null;
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      removingUnmanaged.value = false;
    }
  }

  /** 批量移除:逐项执行,每项执行前重取快照拿到最新 revision(上一次移除会使其失效)。 */
  async function confirmRemove() {
    const targets = removeTargets.value;
    if (!targets.length || removing.value) {
      return;
    }
    removing.value = true;
    let removed = 0;
    let failed = 0;
    try {
      for (const target of targets) {
        try {
          const snapshot = await loadProjectResources(options.projectPath(), options.kind());
          const result = await removeProjectResource({
            path: options.projectPath(),
            kind: options.kind(),
            resourceId: target.id,
            expectedRevision: snapshot.revision,
          });
          if (result.failures.length) {
            failed += result.failures.length;
          } else {
            removed++;
          }
        } catch (e) {
          failed++;
          toast.error(String(e));
        }
      }
      if (failed) {
        toast.error(t("projectAi.partial", { count: removed, failed }));
      } else if (targets.length === 1) {
        toast.success(t("projectAi.removed", { name: targets[0].name }));
      } else {
        toast.success(t("projectAi.removedCount", { count: removed }));
      }
      removeTargets.value = [];
      options.changed();
    } finally {
      removing.value = false;
    }
  }

  return {
    removeTargets,
    removing,
    unmanagedRemoveTarget,
    removingUnmanaged,
    importing,
    toggling,
    toggleAgent,
    configureUnmanaged,
    importLocal,
    importUnmanaged,
    confirmRemove,
    confirmRemoveUnmanaged,
  };
}
