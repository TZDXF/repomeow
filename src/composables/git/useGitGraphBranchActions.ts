import type { ComputedRef, Ref } from "vue";
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

/**
 * 图谱侧栏分支操作状态(驱动行内 spinner 与全量禁用)。
 * - pull/push:本地分支
 * - fetch:远程分支(branch 为空表示「全部更新」)
 * - delete:正在删除的分支(本地或远程)
 */
export interface GitGraphBranchOpState {
  branch: string;
  op: "pull" | "push" | "fetch" | "delete";
}

export function useGitGraphBranchActions(
  project: ComputedRef<Project | undefined>,
  selectedBranch: Ref<string>,
  /** preserveView 为 true 时保留滚动位置与选中详情,只原子替换图谱数据 */
  reload: (preserveView?: boolean) => void,
) {
  const { t } = useI18n(),
    store = useProjectsStore(),
    branchOp = ref<GitGraphBranchOpState | null>(null),
    conflictOpen = ref(false),
    conflictFiles = ref<string[]>([]),
    deleteOpen = ref(false),
    deleteTarget = ref(""),
    deleteIsRemote = ref(false),
    deleteNeedsForce = ref(false),
    deleting = ref(false);

  async function pullBranch(name: string): Promise<boolean> {
    const currentProject = project.value;
    if (!currentProject || branchOp.value) {
      return false;
    }

    branchOp.value = { branch: name, op: "pull" };
    try {
      const conflicts = await store.pullRepository(currentProject, name);
      if (conflicts.length) {
        conflictFiles.value = conflicts;
        conflictOpen.value = true;
        return false;
      }
      toast.success(t("git.pull.success"));
      reload();
      return true;
    } catch (error) {
      toast.error(String(error));
      return false;
    } finally {
      branchOp.value = null;
    }
  }

  async function pushBranch(name: string) {
    const currentProject = project.value;
    if (!currentProject || branchOp.value) {
      return;
    }

    branchOp.value = { branch: name, op: "push" };
    try {
      await store.pushRepository(currentProject, name);
      toast.success(t("git.push.success"));
      reload();
    } catch (error) {
      const code = (error as Error & { code?: string }).code;
      if (code === "git_push_rejected") {
        toast.error(t("git.push.rejected"), {
          action: { label: t("git.push.pullAndPush"), onClick: () => pullThenPushBranch(name) },
        });
      } else {
        toast.error(String(error));
      }
    } finally {
      branchOp.value = null;
    }
  }

  async function pullThenPushBranch(name: string) {
    if (await pullBranch(name)) {
      await pushBranch(name);
    }
  }

  /**
   * 更新远程分支的远端跟踪引用(界面上「更新」):不切换工作区、不合并,只是把
   * origin/xxx 刷新到远端最新提交,让列表与筛选不再基于陈旧快照。
   * name 为空表示全部更新(fetch --prune --all,同时清理远端已删除分支的陈旧引用)。
   */
  async function fetchRemoteBranch(name: string) {
    const currentProject = project.value;
    if (!currentProject || branchOp.value) {
      return;
    }

    branchOp.value = { branch: name, op: "fetch" };
    try {
      if (name) {
        await store.fetchRemoteBranch(currentProject, name);
      } else {
        await store.fetchAllRemoteBranches(currentProject);
      }
      toast.success(
        name ? t("git.branch.fetchSuccess", { name }) : t("git.branch.fetchAllSuccess"),
      );
      // 仅刷新远端跟踪引用:保留阅读位置与当前选中详情
      reload(true);
    } catch (error) {
      toast.error(String(error));
      // 远端分支已删除:刷新一次列表,清掉陈旧跟踪引用,避免展示不存在的分支
      const code = (error as Error & { code?: string }).code;
      if (code === "git_remote_branch_gone") {
        reload(true);
      }
    } finally {
      branchOp.value = null;
    }
  }

  function askDeleteBranch(name: string) {
    deleteTarget.value = name;
    deleteIsRemote.value = false;
    deleteNeedsForce.value = false;
    deleteOpen.value = true;
  }

  /** 删除远程分支:无 force 流程(远端删除不会丢本地数据) */
  function askDeleteRemoteBranch(name: string) {
    deleteTarget.value = name;
    deleteIsRemote.value = true;
    deleteNeedsForce.value = false;
    deleteOpen.value = true;
  }

  async function confirmDeleteBranch() {
    const currentProject = project.value,
      name = deleteTarget.value,
      remote = deleteIsRemote.value;
    if (!currentProject || !name || deleting.value) {
      return;
    }

    deleting.value = true;
    branchOp.value = { branch: name, op: "delete" };
    try {
      if (remote) {
        await store.deleteRemoteBranch(currentProject, name);
        toast.success(t("git.branch.remoteDeleted", { name }));
      } else {
        await store.deleteBranch(currentProject, name, deleteNeedsForce.value);
        toast.success(t("git.branch.deleted", { name }));
      }
      deleteOpen.value = false;
      if (selectedBranch.value === name) {
        selectedBranch.value = "";
      }
      reload();
    } catch (error) {
      const code = (error as Error & { code?: string }).code;
      if (!remote && code === "git_branch_not_merged" && !deleteNeedsForce.value) {
        deleteNeedsForce.value = true;
      } else {
        toast.error(String(error));
        deleteOpen.value = false;
      }
    } finally {
      deleting.value = false;
      branchOp.value = null;
    }
  }

  return {
    askDeleteBranch,
    askDeleteRemoteBranch,
    branchOp,
    confirmDeleteBranch,
    conflictFiles,
    conflictOpen,
    deleteIsRemote,
    deleteNeedsForce,
    deleteOpen,
    deleteTarget,
    deleting,
    fetchRemoteBranch,
    pullBranch,
    pushBranch,
  };
}
