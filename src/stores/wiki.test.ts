import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { i18n } from "@/i18n";
import { regenerateWikiPage, updateWiki } from "@/lib/wiki-generator";
import { deleteWiki, loadWiki } from "@/lib/wiki";
import type { WikiData } from "@/types";
import { toFriendlyWikiGenerationError, useWikiStore } from "@/stores/wiki";

const generationHarness = vi.hoisted(() => {
  const finishes = new Map<string, () => void>();
  const errors = new Map<string, Error>();
  return { errors, finishes };
});

const localStorageData = new Map<string, string>();

beforeEach(() => {
  localStorageData.clear();
  vi.stubGlobal("localStorage", {
    getItem: vi.fn((key: string) => localStorageData.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => localStorageData.set(key, value)),
  });
});

vi.mock("@/lib/wiki-generator", () => ({
  regenerateWikiPage: vi.fn(),
  updateWiki: vi.fn(),
  generateWiki: vi.fn(
    async (
      ...[project, _options, _signal, callbacks]: [
        { path: string },
        unknown,
        AbortSignal,
        { onPhase: (phase: string) => void },
      ]
    ) => {
      callbacks.onPhase("collecting");
      await new Promise<void>((resolve) => {
        generationHarness.finishes.set(project.path, resolve);
      });
      const error = generationHarness.errors.get(project.path);
      if (error) {
        callbacks.onPhase("failed");
        throw error;
      }
      callbacks.onPhase("done");
    },
  ),
}));

vi.mock("@/lib/wiki", () => ({
  deleteWiki: vi.fn(),
  loadWiki: vi.fn(async () => null),
}));

vi.mock("@/stores/settings", () => ({
  useSettingsStore: () => ({
    aiConcurrency: 2,
    aiModel: "test-model",
  }),
}));

describe("wiki store generation concurrency", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
    i18n.global.locale.value = "zh-CN";
    generationHarness.errors.clear();
    generationHarness.finishes.clear();
    vi.mocked(loadWiki).mockResolvedValue(null);
    vi.mocked(regenerateWikiPage).mockResolvedValue({
      model: "test-model",
      generator: "builtin",
    });
  });

  it("allows different projects to generate independently", async () => {
    const store = useWikiStore();
    const first = store.generate({ id: 1, path: "D:\\repos\\first", name: "first" }, "zh-CN");
    const second = store.generate({ id: 2, path: "D:\\repos\\second", name: "second" }, "zh-CN");

    expect(store.isGenerating("D:\\repos\\first")).toBe(true);
    expect(store.isGenerating("D:\\repos\\second")).toBe(true);
    expect(store.backgroundTasks).toHaveLength(2);
    expect(store.backgroundTasks).toEqual(
      expect.arrayContaining([expect.objectContaining({ projectId: 1, action: "generate" })]),
    );

    generationHarness.finishes.get("D:\\repos\\first")?.();
    await first;

    expect(store.generationFor("D:\\repos\\first")?.phase).toBe("done");
    expect(store.isGenerating("D:\\repos\\first")).toBe(false);
    expect(store.isGenerating("D:\\repos\\second")).toBe(true);
    expect(store.backgroundTasks).toHaveLength(1);

    generationHarness.finishes.get("D:\\repos\\second")?.();
    await second;

    expect(store.generationFor("D:\\repos\\second")?.phase).toBe("done");
    expect(store.generating).toBe(false);
    expect(store.backgroundTasks).toHaveLength(0);
  });

  it("将大纲解析错误转换为友好的用户提示", async () => {
    const store = useWikiStore();
    const path = "D:\\repos\\invalid-outline";
    generationHarness.errors.set(path, new Error("wiki outline JSON validation failed"));

    const run = store.generate({ path, name: "invalid-outline" }, "zh-CN");
    generationHarness.finishes.get(path)?.();
    await run;

    expect(store.generationFor(path)?.error).toBe(
      "AI 返回的大纲格式不完整。请重试生成；如果多次失败，请更换模型。",
    );
  });

  it("将模型 token 上限错误转换为可操作的提示", () => {
    expect(
      toFriendlyWikiGenerationError(
        "code=AiMaxOutputTokensExceeded message=invalid params, model[MiniMax-M3] does not support max tokens > 524288 (2013) &#x20;",
      ),
    ).toBe("AI 请求设置的最大输出 Token 数超过当前模型上限，请调整 Agent 配置或更换模型后重试");
  });

  it("将限流和服务临时异常转换为友好提示", () => {
    expect(toFriendlyWikiGenerationError("code=AiRateLimited message=HTTP 429")).toBe(
      "AI 请求过于频繁，已触发服务商限流",
    );
    expect(toFriendlyWikiGenerationError("code=AiServiceUnavailable message=HTTP 503")).toBe(
      "AI 服务暂时不可用，请稍后重试",
    );
  });
});

describe("wiki store automatic update", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
    vi.mocked(regenerateWikiPage).mockResolvedValue({
      model: "test-model",
      generator: "builtin",
    });
  });

  it("HEAD 变化后把自动增量任务整体交给后端读取项目配置", async () => {
    vi.mocked(updateWiki).mockResolvedValue({ updatedPageIds: ["overview"] });
    const project = { path: "D:\\repos\\wiki-auto-update", name: "wiki-auto-update" };

    const count = await useWikiStore().autoUpdate(project, "zh-CN");

    expect(count).toBe(1);
    expect(updateWiki).toHaveBeenCalledWith(
      project,
      expect.not.objectContaining({ backend: expect.anything() }),
      true,
      expect.any(Function),
    );
    expect(regenerateWikiPage).not.toHaveBeenCalled();
  });

  it("后端判定没有受影响页面时返回 0", async () => {
    vi.mocked(updateWiki).mockResolvedValue({ updatedPageIds: [] });
    const project = { path: "D:\\repos\\wiki-auto-update", name: "wiki-auto-update" };

    const count = await useWikiStore().autoUpdate(project, "zh-CN");

    expect(count).toBe(0);
    expect(regenerateWikiPage).not.toHaveBeenCalled();
    expect(updateWiki).toHaveBeenCalledOnce();
  });

  it("把增量更新页面持久化为未读，打开页面后清除", async () => {
    vi.mocked(updateWiki).mockResolvedValue({ updatedPageIds: ["overview", "architecture"] });
    const project = { path: "D:\\repos\\wiki-auto-update", name: "wiki-auto-update" };

    await useWikiStore().autoUpdate(project, "zh-CN");
    expect(useWikiStore().isPageUnread(project.path, "overview")).toBe(true);

    setActivePinia(createPinia());
    const restored = useWikiStore();
    expect(restored.isPageUnread(project.path, "architecture")).toBe(true);

    restored.markPageRead(project.path, "architecture");
    expect(restored.isPageUnread(project.path, "architecture")).toBe(false);
    expect(restored.isPageUnread(project.path, "overview")).toBe(true);
  });
});

function document(path: string, content = path): WikiData {
  return {
    meta: {
      version: 1,
      projectPath: path,
      generatedAt: "2026-10-17",
      headSha: null,
      model: "test",
      language: "zh-CN",
      status: "done",
      outline: [],
    },
    pages: [
      {
        id: "overview",
        file: "overview.md",
        title: content,
        description: "",
        section: null,
        importance: "high",
        relevantFiles: [],
        relatedPages: [],
        content,
      },
    ],
    stale: false,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe("缓存 Wiki 页面的项目隔离", () => {
  const first = { path: "C:/repos/first", name: "first" };
  const second = { path: "C:/repos/second", name: "second" };
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
    vi.mocked(loadWiki).mockImplementation(async (path) => document(path));
    vi.mocked(deleteWiki).mockResolvedValue(undefined);
    vi.mocked(updateWiki).mockResolvedValue({ updatedPageIds: ["overview"] });
    generationHarness.finishes.clear();
    generationHarness.errors.clear();
  });

  it("A → B → A 始终保留各自文档,归一化路径复用同一阅读状态", async () => {
    const store = useWikiStore();
    const a = store.documentFor(first.path);
    await store.load(first.path);
    await store.load(second.path);
    expect(a.data?.meta.projectPath).toBe(first.path);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
    expect(store.documentFor(" C:/repos/first/ ")).toBe(a);
    await store.load(first.path);
    expect(a.data?.meta.projectPath).toBe(first.path);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });

  it("不同项目并发加载和乱序完成不会共享 loading 或覆盖内容", async () => {
    const pendingA = deferred<WikiData>();
    const pendingB = deferred<WikiData>();
    vi.mocked(loadWiki).mockReturnValueOnce(pendingA.promise).mockReturnValueOnce(pendingB.promise);
    const store = useWikiStore();
    const a = store.load(first.path);
    const b = store.load(second.path);
    expect(store.documentFor(first.path).loading).toBe(true);
    expect(store.documentFor(second.path).loading).toBe(true);
    pendingB.resolve(document(second.path));
    await b;
    expect(store.documentFor(first.path).loading).toBe(true);
    expect(store.documentFor(second.path).loading).toBe(false);
    pendingA.resolve(document(first.path));
    await a;
    expect(store.documentFor(first.path).data?.meta.projectPath).toBe(first.path);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });

  it("同一项目迟到的旧响应不能覆盖新内容或提前结束 loading", async () => {
    const older = deferred<WikiData>();
    const newer = deferred<WikiData>();
    vi.mocked(loadWiki).mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    const store = useWikiStore();
    const oldRun = store.load(first.path);
    const newRun = store.load(first.path);
    older.resolve(document(first.path, "old"));
    await oldRun;
    expect(store.documentFor(first.path).loading).toBe(true);
    newer.resolve(document(first.path, "new"));
    await newRun;
    expect(store.documentFor(first.path).data?.pages[0].content).toBe("new");
    const delayed = deferred<WikiData>();
    vi.mocked(loadWiki).mockReturnValueOnce(delayed.promise);
    const lateRun = store.load(first.path);
    await store.load(first.path);
    delayed.resolve(document(first.path, "obsolete"));
    await lateRun;
    expect(store.documentFor(first.path).data?.pages[0].content).toBe(first.path);
  });

  it("刷新失败保留该项目已有文档,不影响另一个项目", async () => {
    const store = useWikiStore();
    await store.load(first.path);
    await store.load(second.path);
    vi.mocked(loadWiki).mockRejectedValueOnce(new Error("read failed"));
    await expect(store.load(first.path)).rejects.toThrow("read failed");
    expect(store.documentFor(first.path).data?.meta.projectPath).toBe(first.path);
    expect(store.documentFor(first.path).loading).toBe(false);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });

  it("删除只清理目标项目,迟到的读取不能复活已删除文档", async () => {
    const store = useWikiStore();
    await store.load(first.path);
    await store.load(second.path);
    const pending = deferred<WikiData>();
    vi.mocked(loadWiki).mockReturnValueOnce(pending.promise);
    const run = store.load(first.path);
    await store.remove(first.path);
    expect(store.documentFor(first.path)).toEqual({ data: null, loading: false });
    pending.resolve(document(first.path));
    await run;
    expect(store.documentFor(first.path).data).toBeNull();
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });

  it("后台更新已缓存的 A 只刷新 A,不会覆盖 B,且保留 A 的未读提示", async () => {
    const store = useWikiStore();
    await store.load(first.path);
    await store.load(second.path);
    vi.mocked(loadWiki).mockClear();
    await store.autoUpdate(first, "zh-CN");
    expect(loadWiki).toHaveBeenCalledExactlyOnceWith(first.path);
    expect(store.documentFor(first.path).data?.meta.projectPath).toBe(first.path);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
    expect(store.isPageUnread(first.path, "overview")).toBe(true);
  });

  it("后台整本生成结束后刷新所属项目的缓存", async () => {
    const store = useWikiStore();
    await store.load(first.path);
    await store.load(second.path);
    vi.mocked(loadWiki).mockClear();
    const run = store.generate(first, "zh-CN");
    generationHarness.finishes.get(first.path)?.();
    await run;
    expect(loadWiki).toHaveBeenCalledExactlyOnceWith(first.path);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });

  it("单页重生成使用所属项目的页面,完成后只刷新该项目", async () => {
    const store = useWikiStore();
    await store.load(first.path);
    await store.load(second.path);
    const page = store.documentFor(first.path).data!.pages[0];
    vi.mocked(loadWiki).mockClear();
    await store.regeneratePage(first, page, "zh-CN");
    expect(regenerateWikiPage).toHaveBeenCalledWith(first, page, "zh-CN", expect.any(AbortSignal));
    expect(loadWiki).toHaveBeenCalledExactlyOnceWith(first.path);
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });

  it("另一个项目已有文档不应让无文档项目进入手动更新", async () => {
    const store = useWikiStore();
    await store.load(second.path);
    expect(await store.update(first, "zh-CN")).toBe(0);
    expect(updateWiki).not.toHaveBeenCalled();
    await store.load(first.path);
    await store.update(first, "zh-CN");
    expect(updateWiki).toHaveBeenCalledWith(first, expect.any(Object), false, expect.any(Function));
    expect(store.documentFor(second.path).data?.meta.projectPath).toBe(second.path);
  });
});
