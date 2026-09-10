import { computed, reactive } from "vue";
import { defineStore } from "pinia";
import { toast } from "vue-sonner";
import { i18n, type SupportedLocale } from "@/i18n";
import { generateAgentsMd } from "@/lib/ai";
import { cleanPath } from "@/lib/path";
import type { Project } from "@/types";

export interface AgentsMdGenerationState {
  projectId: number;
  projectName: string;
  projectPath: string;
  status: "running" | "done" | "failed" | "cancelled";
  startedAt: number;
  finishedAt: number | null;
}

/**
 * AGENTS.md 生成的全局任务状态(对齐 wiki/batch-report 模式):
 * 任务挂在 Pinia 而非组件上,离开项目页后生成继续、完成/失败照样 toast,
 * 标题栏任务中心经 backgroundTasks 镜像进度;回到页面时可按状态恢复展示。
 */
export const useAgentsMdStore = defineStore("agentsMd", () => {
  /** 按项目路径(归一化)记录最近一次生成;同项目同时只允许一个任务 */
  const generations = reactive<Record<string, AgentsMdGenerationState>>({});
  const controllers = new Map<string, AbortController>();

  /** 获取指定项目的生成状态;返回值由 Vue reactive 托管,可直接用于 computed/watch */
  function generationFor(projectPath: string): AgentsMdGenerationState | undefined {
    return generations[cleanPath(projectPath)];
  }

  function isGenerating(projectPath: string): boolean {
    return generationFor(projectPath)?.status === "running";
  }

  /** 标题栏任务中心镜像:进行中的生成(不定进度,total=0) */
  const backgroundTasks = computed(() =>
    Object.values(generations)
      .filter((state) => state.status === "running")
      .map((state) => ({
        id: `agents-md:${cleanPath(state.projectPath)}`,
        projectId: state.projectId,
        projectName: state.projectName,
      })),
  );

  /** 生成/重新生成;成功/失败在此 toast(页面可能已离开),取消静默收场。 */
  async function generate(
    project: Project,
    language: SupportedLocale,
    options: { model?: string; thinking?: string },
  ): Promise<void> {
    const key = cleanPath(project.path);
    if (generations[key]?.status === "running") {
      return;
    }
    const controller = new AbortController();
    controllers.set(key, controller);
    const state = reactive<AgentsMdGenerationState>({
      projectId: project.id,
      projectName: project.name,
      projectPath: project.path,
      status: "running",
      startedAt: Date.now(),
      finishedAt: null,
    });
    generations[key] = state;
    try {
      const result = await generateAgentsMd(project, language, {
        ...options,
        signal: controller.signal,
      });
      if (!result) {
        state.status = "cancelled";
        return;
      }
      state.status = "done";
      let note = "";
      if (result.claudeAction === "created") {
        note = i18n.global.t("aiAssets.agentsMdClaudeCreated");
      } else if (result.claudeAction === "aligned") {
        note = i18n.global.t("aiAssets.agentsMdClaudeAligned");
      }
      const done = i18n.global.t("aiAssets.agentsMdDone");
      toast.success(note ? `${done} · ${note}` : done);
    } catch (e) {
      state.status = "failed";
      toast.error(String(e));
    } finally {
      controllers.delete(key);
      state.finishedAt = Date.now();
    }
  }

  /** 用户主动取消:中止 AI 请求,已写出的内容保留 */
  function cancel(projectPath: string) {
    controllers.get(cleanPath(projectPath))?.abort();
  }

  return { generations, backgroundTasks, generationFor, isGenerating, generate, cancel };
});
