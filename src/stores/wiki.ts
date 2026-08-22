import { computed, reactive, ref } from "vue";
import { defineStore } from "pinia";
import { i18n, type SupportedLocale } from "@/i18n";
import {
  backendIdOf,
  createWikiKernel,
  generateWiki,
  regenerateWikiPage,
  type WikiGenBackend,
  type WikiGenKernel,
  type WikiGenOptions,
  type WikiGenPhase,
  type WikiPageStatus,
} from "@/lib/wiki-generator";
import { commitWiki, deleteWiki, loadWiki, saveWikiMeta, wikiChangedFiles } from "@/lib/wiki";
import { cleanPath } from "@/lib/path";
import { useSettingsStore } from "@/stores/settings";
import type { WikiChangedFiles, WikiData, WikiOutlinePage } from "@/types";

/** 生成进度中的单页状态(进度 UI 直接渲染) */
export interface WikiGenPageItem {
  page: WikiOutlinePage;
  status: WikiPageStatus;
  error?: string;
}

/** 单个项目的整本生成状态;不同项目各自持有一份,可同时生成 */
export interface WikiGenerationState {
  phase: WikiGenPhase | "idle";
  pages: WikiGenPageItem[];
  error: string;
  streamContents: Record<string, string>;
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
  if (raw.startsWith("wiki outline:")) {
    return i18n.global.t("wiki.invalidOutline");
  }
  return raw;
}

/** 按设置组装生成选项:内置 API 或本地 agent(ACP 会话)后端 */
function buildGenOptions(language: SupportedLocale): WikiGenOptions {
  const settings = useSettingsStore();
  const backend: WikiGenBackend =
    settings.wikiGenBackend === "builtin"
      ? { kind: "builtin" }
      : settings.wikiGenBackend === "custom"
        ? {
            kind: "agent",
            customCommand: settings.wikiAgentCustomCommand,
            model: settings.wikiAgentModel || undefined,
            thinking: settings.wikiAgentThinking || undefined,
          }
        : {
            kind: "agent",
            agentId: settings.wikiGenBackend,
            model: settings.wikiAgentModel || undefined,
            thinking: settings.wikiAgentThinking || undefined,
          };
  return {
    language,
    concurrency: settings.aiConcurrency,
    model: settings.aiModel,
    backend,
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
  function generate(project: { path: string; name: string }, language: SupportedLocale) {
    const key = generationKey(project.path);
    if (isGenerating(project.path)) return generationRuns.get(key) ?? Promise.resolve();
    const run = runGenerate(project, language, key);
    generationRuns.set(key, run);
    return run;
  }

  async function runGenerate(
    project: { path: string; name: string },
    language: SupportedLocale,
    key: string,
  ) {
    const controller = new AbortController();
    const state = reactive<WikiGenerationState>({
      phase: "idle",
      pages: [],
      error: "",
      streamContents: {},
    });
    generations[key] = state;
    generationControllers.set(key, controller);
    try {
      await generateWiki(project, buildGenOptions(language), controller.signal, {
        onPhase: (p) => {
          state.phase = p;
        },
        onPage: (page, status, error) => {
          const item = state.pages.find((i) => i.page.id === page.id);
          if (item) {
            item.status = status;
            item.error = error;
          } else {
            state.pages.push({ page: { ...page }, status, error });
          }
          // 页面进入终态后清掉流式预览内容
          if (status !== "running" && status !== "pending") {
            delete state.streamContents[page.id];
          }
        },
        onPageProgress: (page, partial) => {
          state.streamContents[page.id] = partial;
        },
      });
    } catch (e) {
      state.error = toFriendlyWikiGenerationError(e);
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
  async function regeneratePage(
    project: { path: string; name: string },
    page: WikiOutlinePage,
    language: SupportedLocale,
  ) {
    if (isGenerating(project.path) || regeneratingPage.value) return;
    regeneratingPage.value = page.id;
    // agent 后端为本次重生成单独起会话(原生成会话早已结束),完成后即收尾
    let kernel: WikiGenKernel | null = null;
    try {
      kernel = await createWikiKernel(project, buildGenOptions(language));
      await regenerateWikiPage(kernel, project.path, page, language, new AbortController().signal);
      // git 快照提交(辅助管理,失败不影响页面内容,下次操作会补提交)
      await commitWiki(project.path, "page", page.title).catch(() => {});
      if (dataFor.value === project.path) await load(project.path);
    } finally {
      regeneratingPage.value = null;
      await kernel?.dispose().catch(() => {});
    }
  }

  /**
   * 增量更新:取 meta.headSha..HEAD 的变更文件,只重生成相关文件命中的页面,
   * 成功后把 meta.headSha 推进到当前 HEAD。返回重生成的页数;
   * 无 headSha(非 git 项目)或历史被改写时抛错,由调用方退化为整本重生成
   */
  const updating = ref(false);
  async function update(
    project: { path: string; name: string },
    language: SupportedLocale,
  ): Promise<number> {
    const d = data.value;
    if (!d || updating.value || isGenerating(project.path)) return 0;
    const fromSha = d.meta.headSha;
    if (!fromSha) throw new Error("no head sha");
    const options = buildGenOptions(language);
    // 跨后端(如内置 API ↔ agent)的旧 wiki 不做增量:抛错由调用方退化为整本重生成
    if ((d.meta.generator ?? "builtin") !== backendIdOf(options.backend)) {
      throw new Error("generator mismatch");
    }
    updating.value = true;
    try {
      const changed = await wikiChangedFiles(project.path, fromSha);
      return await applyUpdate(project, d, language, changed, options);
    } finally {
      updating.value = false;
      regeneratingPage.value = null;
    }
  }

  /** 增量更新核心:重生成 changed 命中的页面并推进 meta.headSha(调用方持有 updating 标记) */
  async function applyUpdate(
    project: { path: string; name: string },
    d: WikiData,
    language: SupportedLocale,
    changed: WikiChangedFiles,
    options: WikiGenOptions,
  ): Promise<number> {
    const changedSet = new Set(changed.files);
    const affected = d.pages.filter((p) => p.relevantFiles.some((f) => changedSet.has(f)));
    let kernel: WikiGenKernel | null = null;
    try {
      kernel = await createWikiKernel(project, options);
      for (const page of affected) {
        regeneratingPage.value = page.id;
        // 页数少(通常个位数),串行足够;保持与 regeneratePage 同一互斥标记
        // eslint-disable-next-line no-await-in-loop
        await regenerateWikiPage(
          kernel,
          project.path,
          page,
          language,
          new AbortController().signal,
          { changedFiles: changed.files },
        );
      }
      regeneratingPage.value = null;
      if (changed.headSha) {
        await saveWikiMeta(
          project.path,
          { ...d.meta, headSha: changed.headSha, model: kernel.model, generator: kernel.backendId },
          "update",
        );
      }
    } finally {
      await kernel?.dispose().catch(() => {});
    }
    if (dataFor.value === project.path) await load(project.path);
    return affected.length;
  }

  /**
   * 后台自动增量更新(「跟踪更新」联动):auto-pull 快进成功事件触发。
   * 是否参与由调用方按两级开关决定(全局开 = 所有的跟踪项目,全局关 = 仅项目勾选的),
   * 这里只看运行条件:正在生成或更新/无 wiki/无 headSha/提交数未达阈值时静默跳过
   * (返回 0);headSha 已不在当前历史(改写)同样跳过,整本重生成只留给用户手动触发。
   * 内部串行排队(多个项目的触发依次执行);正忙时本次跳过,后续拉取事件会再次触发。
   * 执行失败向外抛,由调用方提示
   */
  let autoQueue: Promise<unknown> = Promise.resolve();
  function autoUpdate(project: { path: string; name: string }, language: SupportedLocale) {
    const run = autoQueue.then(() => runAutoUpdate(project, language));
    // 队列自身吞掉异常继续前进;错误经返回的 promise 交给触发方
    autoQueue = run.then(
      () => {},
      () => {},
    );
    return run;
  }

  async function runAutoUpdate(
    project: { path: string; name: string },
    language: SupportedLocale,
  ): Promise<number> {
    if (updating.value || isGenerating(project.path) || regeneratingPage.value) {
      return 0;
    }
    const options = buildGenOptions(language);
    // agent 后端不参与自动增量更新:后台无人值守跑 agent 会静默消耗订阅额度,
    // 增量更新只留给用户在 wiki 页手动触发
    if (options.backend.kind !== "builtin") return 0;
    const d = await loadWiki(project.path).catch(() => null);
    const fromSha = d?.meta.headSha;
    if (!d || !fromSha) return 0;
    // 跨后端的旧 wiki 不自动增量(手动更新会退化为整本重生成),静默跳过
    if ((d.meta.generator ?? "builtin") !== backendIdOf(options.backend)) return 0;
    const changed = await wikiChangedFiles(project.path, fromSha).catch(() => null);
    if (!changed) return 0;
    const settings = useSettingsStore();
    if (changed.commitCount < settings.wikiAutoUpdateThreshold) return 0;
    updating.value = true;
    try {
      return await applyUpdate(project, d, language, changed, options);
    } finally {
      updating.value = false;
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
