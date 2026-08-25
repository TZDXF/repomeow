import type { ComputedRef, Ref } from "vue";
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { toast } from "vue-sonner";
import { useProjectsStore } from "@/stores/projects";
import type { Project } from "@/types";

export function useGitGraphBranchActions(
  project: ComputedRef<Project | undefined>,
  selectedBranch: Ref<string>,
  reload: () => void,
) {
  const { t } = useI18n();
  const store = useProjectsStore();
  const branchOp = ref<{ branch: string; op: "pull" | "push" } | null>(null);
  const conflictOpen = ref(false);
  const conflictFiles = ref<string[]>([]);
  const deleteOpen = ref(false);
  const deleteTarget = ref("");
  const deleteNeedsForce = ref(false);
  const deleting = ref(false);

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

  function askDeleteBranch(name: string) {
    deleteTarget.value = name;
    deleteNeedsForce.value = false;
    deleteOpen.value = true;
  }

  async function confirmDeleteBranch() {
    const currentProject = project.value;
    const name = deleteTarget.value;
    if (!currentProject || !name || deleting.value) {
      return;
    }

    deleting.value = true;
    try {
      await store.deleteBranch(currentProject, name, deleteNeedsForce.value);
      toast.success(t("git.branch.deleted", { name }));
      deleteOpen.value = false;
      if (selectedBranch.value === name) {
        selectedBranch.value = "";
      }
      reload();
    } catch (error) {
      const code = (error as Error & { code?: string }).code;
      if (code === "git_branch_not_merged" && !deleteNeedsForce.value) {
        deleteNeedsForce.value = true;
      } else {
        toast.error(String(error));
        deleteOpen.value = false;
      }
    } finally {
      deleting.value = false;
    }
  }

  return {
    askDeleteBranch,
    branchOp,
    confirmDeleteBranch,
    conflictFiles,
    conflictOpen,
    deleteNeedsForce,
    deleteOpen,
    deleteTarget,
    deleting,
    pullBranch,
    pushBranch,
  };
}
