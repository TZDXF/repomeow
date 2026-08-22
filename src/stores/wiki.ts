import { computed, ref } from "vue";
import { defineStore } from "pinia";
import type { SupportedLocale } from "@/i18n";
import {
  generateWiki,
  regenerateWikiPage,
  type WikiGenPhase,
  type WikiPageStatus,
} from "@/lib/wiki-generator";
import { commitWiki, deleteWiki, loadWiki, saveWikiMeta, wikiChangedFiles } from "@/lib/wiki";
import { useSettingsStore } from "@/stores/settings";
import type { WikiData, WikiOutlinePage } from "@/types";

/** 生成进度中的单页状态(进度 UI 直接渲染) */
export interface WikiGenPageItem {
  page: WikiOutlinePage;
  status: WikiPageStatus;
  error?: string;
}

/**
 * 项目 wiki 的查看与生成状态。wiki 文件落盘在 ~/.repomeow/wiki/<目录>。
 * store 是全局单例:**离开 wiki 页面不中止生成**——生成状态(genFor)与加载的
 * wiki 数据(dataFor)各自记录所属项目路径,视图只展示与当前项目匹配的部分;
 * 同时进行中的整本生成只允许一个(AI 管线串行),期间其他项目可正常查看已有 wiki。
 */
export const useWikiStore = defineStore("wiki", () => {
  const data = ref<WikiData | null>(null);
  /** data 所属的项目路径 */
  const dataFor = ref<string | null>(null);
  const loading = ref(false);

  // ── 生成进度 ──
  const phase = ref<WikiGenPhase | "idle">("idle");
  /** 正在生成的项目路径;视图据此判断进度面板是否属于当前项目 */
  const genFor = ref<string | null>(null);
  const pages = ref<WikiGenPageItem[]>([]);
  const genError = ref("");
  /** 逐页流式生成中的正文(pageId → 已产出内容),供进度面板实时预览 */
  const streamContents = ref<Record<string, string>>({});
  let controller: AbortController | null = null;
  /** 进行中的整本生成 promise;remove 时等待其收敛,避免删除后页面又被落盘重建 */
  let genRun: Promise<void> | null = null;

  const generating = computed(() =>
    ["collecting", "outlining", "generating"].includes(phase.value),
  );
  const doneCount = computed(
    () => pages.value.filter((p) => p.status === "done" || p.status === "failed").length,
  );

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

  /** 整本生成(已有生成任务进行中时忽略);结束后若用户仍在看该项目则刷新展示 */
  function generate(project: { path: string; name: string }, language: SupportedLocale) {
    if (generating.value) return Promise.resolve();
    genRun = runGenerate(project, language);
    return genRun;
  }

  async function runGenerate(project: { path: string; name: string }, language: SupportedLocale) {
    if (generating.value) return;
    const settings = useSettingsStore();
    controller = new AbortController();
    genFor.value = project.path;
    pages.value = [];
    genError.value = "";
    streamContents.value = {};
    try {
      await generateWiki(
        project,
        {
          language,
          concurrency: settings.aiConcurrency,
          model: settings.aiModel,
        },
        controller.signal,
        {
          onPhase: (p) => {
            phase.value = p;
          },
          onPage: (page, status, error) => {
            const item = pages.value.find((i) => i.page.id === page.id);
            if (item) {
              item.status = status;
              item.error = error;
            } else {
              pages.value.push({ page: { ...page }, status, error });
            }
            // 页面进入终态后清掉流式预览内容
            if (status !== "running" && status !== "pending") {
              delete streamContents.value[page.id];
            }
          },
          onPageProgress: (page, partial) => {
            streamContents.value[page.id] = partial;
          },
        },
      );
    } catch (e) {
      genError.value = e instanceof Error ? e.message : String(e);
    } finally {
      controller = null;
      genRun = null;
      // 取消/失败时已生成的页面文件仍在磁盘(无 meta 则整本无效),统一以落盘状态为准;
      // 用户已切到别的项目时不回写 data,避免覆盖其正在查看的 wiki
      if (dataFor.value === project.path) {
        await load(project.path).catch(() => {});
      }
    }
  }

  /** 用户主动取消(中止 AI 请求与后续页面派发) */
  function cancel() {
    controller?.abort();
  }

  /** 单页重生成:就地更新该页内容,保持其余页面与 meta 不变 */
  const regeneratingPage = ref<string | null>(null);
  async function regeneratePage(
    projectPath: string,
    page: WikiOutlinePage,
    language: SupportedLocale,
  ) {
    if (generating.value || regeneratingPage.value) return;
    regeneratingPage.value = page.id;
    try {
      await regenerateWikiPage(projectPath, page, language, new AbortController().signal);
      // git 快照提交(辅助管理,失败不影响页面内容,下次操作会补提交)
      await commitWiki(projectPath, "page", page.title).catch(() => {});
      if (dataFor.value === projectPath) await load(projectPath);
    } finally {
      regeneratingPage.value = null;
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
    if (!d || updating.value || generating.value) return 0;
    const fromSha = d.meta.headSha;
    if (!fromSha) throw new Error("no head sha");
    updating.value = true;
    try {
      const { files: changed, headSha } = await wikiChangedFiles(project.path, fromSha);
      const changedSet = new Set(changed);
      const affected = d.pages.filter((p) => p.relevantFiles.some((f) => changedSet.has(f)));
      for (const page of affected) {
        regeneratingPage.value = page.id;
        // 页数少(通常个位数),串行足够;保持与 regeneratePage 同一互斥标记
        // eslint-disable-next-line no-await-in-loop
        await regenerateWikiPage(project.path, page, language, new AbortController().signal);
      }
      regeneratingPage.value = null;
      if (headSha) {
        await saveWikiMeta(project.path, { ...d.meta, headSha }, "update");
      }
      if (dataFor.value === project.path) await load(project.path);
      return affected.length;
    } finally {
      updating.value = false;
      regeneratingPage.value = null;
    }
  }

  async function remove(projectPath: string) {
    // 该项目正在生成时先中止并等流水线收敛,避免删除后又被落盘的页面把目录建回来
    if (genRun && genFor.value === projectPath) {
      controller?.abort();
      await genRun.catch(() => {});
    }
    await deleteWiki(projectPath);
    if (dataFor.value === projectPath) data.value = null;
  }

  return {
    data,
    dataFor,
    loading,
    phase,
    genFor,
    pages,
    genError,
    streamContents,
    generating,
    doneCount,
    regeneratingPage,
    updating,
    load,
    generate,
    cancel,
    regeneratePage,
    update,
    remove,
  };
});
