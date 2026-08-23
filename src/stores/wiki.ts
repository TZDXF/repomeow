import { computed, reactive, ref } from "vue";
import { defineStore } from "pinia";
import { i18n, type SupportedLocale } from "@/i18n";
import {
  generateWiki,
  regenerateWikiPage,
  updateWiki,
  type WikiGenOptions,
  type WikiGenPhase,
  type WikiGenerationActivity,
  type WikiContextSummary,
  type WikiPageStatus,
  type WikiRetryStatus,
} from "@/lib/wiki-generator";
import { deleteWiki, loadWiki } from "@/lib/wiki";
import { cleanPath } from "@/lib/path";
import { useSettingsStore } from "@/stores/settings";
import type { WikiData, WikiOutlinePage } from "@/types";

/** 生成进度中的单页状态(进度 UI 直接渲染) */
export interface WikiGenPageItem {
  page: WikiOutlinePage;
  status: WikiPageStatus;
  error?: string;
}

/** 单个项目的整本生成状态;不同项目各自持有一份,可同时生成 */
export interface WikiGenerationState {
  projectId?: number;
  projectName: string;
  phase: WikiGenPhase | "idle";
  pages: WikiGenPageItem[];
  error: string;
  streamContents: Record<string, string>;
  context: WikiContextSummary | null;
  activities: WikiGenerationActivity[];
  retries: Record<string, WikiRetryStatus>;
  /** 本轮整本生成开始时间,供离开页面后返回时继续展示真实耗时 */
  startedAt: number;
}

export interface WikiBackgroundTask {
  id: string;
  action: "generate" | "update" | "page";
  projectId?: number;
  projectName: string;
  completed: number;
  total: number;
}

interface WikiUpdateProgress {
  projectId?: number;
  projectName: string;
  completed: number;
  total: number;
}

interface WikiTaskProject {
  id?: number;
  path: string;
  name: string;
}

function isActiveGeneration(state: WikiGenerationState | undefined): boolean {
  return !!state && ["collecting", "outlining", "generating"].includes(state.phase);
}

/** 生成任务与项目路径一一对应;统一清理尾随分隔符后作为缓存 key */
function generationKey(projectPath: string): string {
  return cleanPath(projectPath);
}

/** 将面向开发者的 Wiki 生成错误转换为适合直接展示给用户的提示。 */
export function toFriendlyWikiGenerationError(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error);
  const normalized = raw.toLowerCase();
  if (normalized.includes("airatelimited") || normalized.includes("ai_rate_limited")) {
    return i18n.global.t("errors.ai_rate_limited");
  }
  if (
    normalized.includes("aiserviceunavailable") ||
    normalized.includes("ai_service_unavailable")
  ) {
    return i18n.global.t("errors.ai_service_unavailable");
  }
  if (
    normalized.includes("aimaxoutputtokensexceeded") ||
    normalized.includes("ai_max_output_tokens_exceeded") ||
    normalized.includes("does not support max tokens >")
  ) {
    return i18n.global.t("errors.ai_max_output_tokens_exceeded");
  }
  if (
    raw.startsWith("wiki outline:") ||
    raw.startsWith("wiki outline JSON parse failed") ||
    raw.startsWith("wiki outline JSON validation failed") ||
    raw.startsWith("wiki outline response must contain")
  ) {
    return i18n.global.t("wiki.invalidOutline");
  }
  return raw;
}

/** 通用生成参数；项目独立的后端配置由 Rust 从 Wiki 目录 config.json 读取。 */
function buildGenOptions(language: SupportedLocale): WikiGenOptions {
  const settings = useSettingsStore();
  return {
    language,
    concurrency: settings.aiConcurrency,
  };
}

/**
 * 项目 wiki 的查看与生成状态。wiki 文件落盘在 ~/.repomeow/wiki/<目录>。
 * store 是全局单例:**离开 wiki 页面不中止生成**——生成状态按项目路径隔离,
 * 不同项目可以同时生成;加载的 wiki 数据(dataFor)仍只对应当前查看的项目。
 */
export const useWikiStore = defineStore("wiki", () => {
  const data = ref<WikiData | null>(null);
  /** data 所属的项目路径 */
  const dataFor = ref<string | null>(null);
  const loading = ref(false);

  // ── 生成进度 ──
  const generations = reactive<Record<string, WikiGenerationState>>({});
  const generationControllers = new Map<string, AbortController>();
  /** 进行中的整本生成 promise;remove 时等待对应项目收敛,避免删除后又被落盘重建 */
  const generationRuns = new Map<string, Promise<void>>();

  /** 获取指定项目的生成状态;返回值由 Vue reactive 托管,可直接用于 computed */
  function generationFor(projectPath: string): WikiGenerationState | undefined {
    return generations[generationKey(projectPath)];
  }

  function isGenerating(projectPath: string): boolean {
    return isActiveGeneration(generationFor(projectPath));
  }

  /** 是否存在任意整本生成任务,供需要全局概览的调用方使用 */
  const generating = computed(() => Object.values(generations).some(isActiveGeneration));

  /** 标题栏使用的 Wiki 后台任务摘要；整本生成、增量更新、单页重生成统一计数。 */
  const backgroundTasks = computed<WikiBackgroundTask[]>(() => {
    const tasks: WikiBackgroundTask[] = Object.entries(generations)
      .filter(([, state]) => isActiveGeneration(state))
      .map(([key, state]) => ({
        id: `generate:${key}`,
        action: "generate",
        projectId: state.projectId,
        projectName: state.projectName,
        completed: state.pages.filter((page) =>
          ["done", "failed", "cancelled"].includes(page.status),
        ).length,
        total: state.pages.length,
      }));
    if (updateProgress.value) {
      tasks.push({
        id: `update:${updateProgress.value.projectName}`,
        action: "update",
        projectId: updateProgress.value.projectId,
        projectName: updateProgress.value.projectName,
        completed: updateProgress.value.completed,
        total: updateProgress.value.total,
      });
    } else if (pageRegeneration.value) {
      tasks.push({
        id: `page:${pageRegeneration.value.projectName}:${pageRegeneration.value.pageTitle}`,
        action: "page",
        projectId: pageRegeneration.value.projectId,
        projectName: pageRegeneration.value.projectName,
        completed: 0,
        total: 1,
      });
    }
    return tasks;
  });

  async function load(projectPath: string) {
    // 切换到另一个项目时先清空,避免旧项目内容闪现
    if (dataFor.value !== projectPath) data.value = null;
    dataFor.value = projectPath;
    loading.value = true;
    try {
      data.value = await loadWiki(projectPath);
    } finally {
      loading.value = false;
    }
  }

  /** 整本生成;同一项目避免重复启动,不同项目可并行;结束后按当前查看项目刷新展示 */
  function generate(project: WikiTaskProject, language: SupportedLocale) {
    const key = generationKey(project.path);
    if (isGenerating(project.path)) return generationRuns.get(key) ?? Promise.resolve();
    const run = runGenerate(project, language, key);
    generationRuns.set(key, run);
    return run;
  }

  async function runGenerate(project: WikiTaskProject, language: SupportedLocale, key: string) {
    const controller = new AbortController();
    const state = reactive<WikiGenerationState>({
      projectId: project.id,
      projectName: project.name,
      phase: "idle",
      pages: [],
      error: "",
      streamContents: {},
      context: null,
      activities: [],
      retries: {},
      startedAt: Date.now(),
    });
    generations[key] = state;
    generationControllers.set(key, controller);
    try {
      await generateWiki(project, buildGenOptions(language), controller.signal, {
        onPhase: (p) => {
          state.phase = p;
          if (p !== "outlining") delete state.retries.outline;
        },
        onPage: (page, status, error) => {
          const item = state.pages.find((i) => i.page.id === page.id);
          if (item) {
            item.status = status;
            item.error = error ? toFriendlyWikiGenerationError(error) : undefined;
          } else {
            state.pages.push({
              page: { ...page },
              status,
              error: error ? toFriendlyWikiGenerationError(error) : undefined,
            });
          }
          // 页面进入终态后清掉流式预览内容
          if (status !== "running" && status !== "pending") {
            delete state.streamContents[page.id];
            delete state.retries[page.id];
          }
        },
        onPageProgress: (page, partial) => {
          state.streamContents[page.id] = partial;
          if (partial) delete state.retries[page.id];
        },
        onContext: (context) => {
          state.context = { ...context };
        },
        onActivities: (activities) => {
          state.activities.push(...activities);
          if (state.activities.length > 2_000) {
            state.activities.splice(0, state.activities.length - 2_000);
          }
        },
        onRetry: (retry) => {
          state.retries[retry.pageId ?? "outline"] = { ...retry };
        },
      });
    } catch (e) {
      state.error = toFriendlyWikiGenerationError(e);
      state.phase = controller.signal.aborted ? "cancelled" : "failed";
    } finally {
      generationControllers.delete(key);
      generationRuns.delete(key);
      // 取消/失败时已生成的页面文件仍在磁盘(无 meta 则整本无效),统一以落盘状态为准;
      // 用户已切到别的项目时不回写 data,避免覆盖其正在查看的 wiki
      if (dataFor.value === project.path) {
        await load(project.path).catch(() => {});
      }
    }
  }

  /** 用户主动取消(中止 AI 请求与后续页面派发) */
  function cancel(projectPath: string) {
    generationControllers.get(generationKey(projectPath))?.abort();
  }

  /** 单页重生成:就地更新该页内容,保持其余页面与 meta 不变 */
  const regeneratingPage = ref<string | null>(null);
  const pageRegeneration = ref<{
    projectId?: number;
    projectName: string;
    pageTitle: string;
  } | null>(null);
  async function regeneratePage(
    project: WikiTaskProject,
    page: WikiOutlinePage,
    language: SupportedLocale,
  ) {
    if (isGenerating(project.path) || regeneratingPage.value) return;
    regeneratingPage.value = page.id;
    pageRegeneration.value = {
      projectId: project.id,
      projectName: project.name,
      pageTitle: page.title,
    };
    try {
      await regenerateWikiPage(project, page, language, new AbortController().signal);
      if (dataFor.value === project.path) await load(project.path);
    } finally {
      regeneratingPage.value = null;
      pageRegeneration.value = null;
    }
  }

  /**
   * 增量更新:取 meta.headSha..HEAD 的变更文件,只重生成相关文件命中的页面,
   * 成功后把 meta.headSha 推进到当前 HEAD。返回重生成的页数;
   * 无 headSha(非 git 项目)或历史被改写时抛错,由调用方退化为整本重生成
   */
  const updating = ref(false);
  const updateProgress = ref<WikiUpdateProgress | null>(null);
  async function update(project: WikiTaskProject, language: SupportedLocale): Promise<number> {
    if (!data.value || updating.value || isGenerating(project.path)) return 0;
    const options = buildGenOptions(language);
    updating.value = true;
    updateProgress.value = {
      projectId: project.id,
      projectName: project.name,
      completed: 0,
      total: 0,
    };
    try {
      const count = await updateWiki(project, options, false, (progress) => {
        if (updateProgress.value) {
          updateProgress.value.completed = progress.completed;
          updateProgress.value.total = progress.total;
        }
      });
      if (dataFor.value === project.path) await load(project.path);
      return count;
    } finally {
      updating.value = false;
      updateProgress.value = null;
      regeneratingPage.value = null;
    }
  }

  /**
   * 后台自动增量更新:统一 Git 事件检测到本地 HEAD 变化后触发。
   * 是否参与由调用方按两级开关决定(全局开 = 所有项目,全局关 = 仅项目勾选的),
   * 这里只看运行条件:正在生成或更新/无 wiki/无 headSha/没有 relevantFiles 命中时静默跳过
   * (返回 0);headSha 已不在当前历史(改写)同样跳过,整本重生成只留给用户手动触发。
   * 内部串行排队(多个项目的触发依次执行);正忙时本次跳过,后续拉取事件会再次触发。
   * 执行失败向外抛,由调用方提示
   */
  let autoQueue: Promise<unknown> = Promise.resolve();
  function autoUpdate(project: WikiTaskProject, language: SupportedLocale) {
    const run = autoQueue.then(() => runAutoUpdate(project, language));
    // 队列自身吞掉异常继续前进;错误经返回的 promise 交给触发方
    autoQueue = run.then(
      () => {},
      () => {},
    );
    return run;
  }

  async function runAutoUpdate(
    project: WikiTaskProject,
    language: SupportedLocale,
  ): Promise<number> {
    if (updating.value || isGenerating(project.path) || regeneratingPage.value) {
      return 0;
    }
    const options = buildGenOptions(language);
    updating.value = true;
    updateProgress.value = {
      projectId: project.id,
      projectName: project.name,
      completed: 0,
      total: 0,
    };
    try {
      const count = await updateWiki(project, options, true, (progress) => {
        if (updateProgress.value) {
          updateProgress.value.completed = progress.completed;
          updateProgress.value.total = progress.total;
        }
      });
      if (dataFor.value === project.path) await load(project.path);
      return count;
    } finally {
      updating.value = false;
      updateProgress.value = null;
      regeneratingPage.value = null;
    }
  }

  async function remove(projectPath: string) {
    // 该项目正在生成时先中止并等流水线收敛,避免删除后又被落盘的页面把目录建回来
    const key = generationKey(projectPath);
    const run = generationRuns.get(key);
    if (run) {
      generationControllers.get(key)?.abort();
      await run.catch(() => {});
    }
    await deleteWiki(projectPath);
    delete generations[key];
    if (dataFor.value === projectPath) data.value = null;
  }

  return {
    data,
    dataFor,
    loading,
    generationFor,
    isGenerating,
    generating,
    backgroundTasks,
    regeneratingPage,
    updating,
    load,
    generate,
    cancel,
    regeneratePage,
    update,
    autoUpdate,
    remove,
  };
});
