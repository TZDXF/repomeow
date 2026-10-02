import { computed, ref, watch, type Ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import {
  assignProjectResource,
  repairProjectResource,
  type ProjectAiTarget,
  type ProjectResourceKind,
  type ProjectResourceSnapshot,
  type RepairAction,
  type ResourceChoice,
  type ResourceDeployment,
} from "@/lib/project-ai-resources";

/** 有明确修复出路的异常状态:应用更新(可更新/缺失)、覆盖更新或强制解除(本地已修改)、强制解除(需检查配置)。 */
export const REPAIRABLE_STATUSES: ResourceDeployment["status"][] = [
  "update",
  "modified",
  "missing",
  "conflict",
];

export function repairableRecordsOf(records: ResourceDeployment[]): ResourceDeployment[] {
  return records.filter((d) => REPAIRABLE_STATUSES.includes(d.status));
}

/** 修复按钮忙碌键:resourceId:agentId:action(update / reapply / detach)。 */
export function repairBusyKey(record: ResourceDeployment, action: "update" | RepairAction): string {
  return `${record.resourceId}:${record.agentId}:${action}`;
}

export interface ResourceRepairOptions {
  projectPath: () => string;
  kind: () => ProjectResourceKind;
  targets: () => ProjectAiTarget[];
  data: Ref<ProjectResourceSnapshot | null>;
  records: (id: string) => ResourceDeployment[];
  changed: () => void;
}

/**
 * 修复面板状态:可修复记录归集、applyUpdate(重走 assign 部署管线)与
 * reapply/detach 修复动作;repairing 为防重入锁,记录全部恢复后自动关面板。
 */
export function useResourceRepair(options: ResourceRepairOptions) {
  const { t } = useI18n();
  const repairResource = ref<ResourceChoice | null>(null);
  const repairing = ref("");
  const repairRecords = computed(() =>
    repairResource.value ? repairableRecordsOf(options.records(repairResource.value.id)) : [],
  );
  watch(repairRecords, (next) => {
    if (repairResource.value && !next.length) {
      repairResource.value = null;
    }
  });
  function repairAgentName(agentId: string) {
    return options.targets().find((a) => a.id === agentId)?.name ?? agentId;
  }
  /** 「可更新/缺失」的修复:以完整目标集合再 assign 一次,走正常部署管线写入来源最新定义。 */
  async function applyUpdate(record: ResourceDeployment) {
    const resource = repairResource.value;
    if (!resource || !options.data.value || repairing.value) {
      return;
    }
    repairing.value = repairBusyKey(record, "update");
    try {
      const current = new Set(options.records(resource.id).map((d) => d.agentId));
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
      } else {
        toast.success(t("projectAi.applied", { count: result.applied }));
      }
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      repairing.value = "";
    }
  }
  /** 「本地已修改/需检查配置」的修复:reapply 覆盖更新(丢弃本地修改),detach 仅解除托管记录、保留项目文件。 */
  async function repairRecord(record: ResourceDeployment, action: RepairAction) {
    if (!options.data.value || repairing.value) {
      return;
    }
    repairing.value = repairBusyKey(record, action);
    try {
      await repairProjectResource({
        path: options.projectPath(),
        kind: options.kind(),
        resourceId: record.resourceId,
        agentId: record.agentId,
        action,
        expectedRevision: options.data.value.revision,
      });
      toast.success(
        t(action === "reapply" ? "projectAi.reapplied" : "projectAi.detached", {
          name: record.name,
        }),
      );
      options.changed();
    } catch (e) {
      toast.error(String(e));
    } finally {
      repairing.value = "";
    }
  }
  return {
    repairResource,
    repairing,
    repairRecords,
    repairAgentName,
    applyUpdate,
    repairRecord,
  };
}
