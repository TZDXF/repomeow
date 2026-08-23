import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { i18n } from "@/i18n";
import { createWikiKernel, regenerateWikiPage, type WikiGenKernel } from "@/lib/wiki-generator";
import { loadWiki, saveWikiMeta, wikiChangedFiles } from "@/lib/wiki";
import { useWikiStore } from "@/stores/wiki";
import type { WikiData, WikiOutlinePage } from "@/types";

const generationHarness = vi.hoisted(() => {
  const finishes = new Map<string, () => void>();
  const errors = new Map<string, Error>();
  return { errors, finishes };
});

vi.mock("@/lib/wiki-generator", () => ({
  backendIdOf: () => "builtin",
  createWikiKernel: vi.fn(),
  regenerateWikiPage: vi.fn(),
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
  commitWiki: vi.fn(),
  deleteWiki: vi.fn(),
  loadWiki: vi.fn(async () => null),
  saveWikiMeta: vi.fn(),
  wikiChangedFiles: vi.fn(),
}));

vi.mock("@/stores/settings", () => ({
  useSettingsStore: () => ({
    aiConcurrency: 2,
    aiModel: "test-model",
    wikiAgentCustomCommand: "",
    wikiAgentModel: "",
    wikiAgentThinking: "",
    wikiGenBackend: "builtin",
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
    vi.mocked(saveWikiMeta).mockResolvedValue(undefined);
    vi.mocked(regenerateWikiPage).mockResolvedValue(undefined);
    vi.mocked(createWikiKernel).mockResolvedValue({
      backendId: "builtin",
      concurrency: 1,
      model: "test-model",
      generateOutline: vi.fn(),
      generatePage: vi.fn(),
      dispose: vi.fn().mockResolvedValue(undefined),
    } satisfies WikiGenKernel);
  });

  it("allows different projects to generate independently", async () => {
    const store = useWikiStore();
    const first = store.generate({ path: "D:\\repos\\first", name: "first" }, "zh-CN");
    const second = store.generate({ path: "D:\\repos\\second", name: "second" }, "zh-CN");

    expect(store.isGenerating("D:\\repos\\first")).toBe(true);
    expect(store.isGenerating("D:\\repos\\second")).toBe(true);

    generationHarness.finishes.get("D:\\repos\\first")?.();
    await first;

    expect(store.generationFor("D:\\repos\\first")?.phase).toBe("done");
    expect(store.isGenerating("D:\\repos\\first")).toBe(false);
    expect(store.isGenerating("D:\\repos\\second")).toBe(true);

    generationHarness.finishes.get("D:\\repos\\second")?.();
    await second;

    expect(store.generationFor("D:\\repos\\second")?.phase).toBe("done");
    expect(store.generating).toBe(false);
  });

  it("将大纲解析错误转换为友好的用户提示", async () => {
    const store = useWikiStore();
    const path = "D:\\repos\\invalid-outline";
    generationHarness.errors.set(path, new Error("wiki outline: no <wiki_structure> found"));

    const run = store.generate({ path, name: "invalid-outline" }, "zh-CN");
    generationHarness.finishes.get(path)?.();
    await run;

    expect(store.generationFor(path)?.error).toBe(
      "AI 返回的大纲格式不完整。请重试生成；如果多次失败，请更换模型或生成后端。",
    );
  });
});

function createWikiData(): WikiData {
  const pages: Array<WikiOutlinePage & { content: string }> = [
    {
      id: "overview",
      file: "01-overview.md",
      title: "Overview",
      description: "Project overview",
      section: null,
      importance: "high",
      relevantFiles: ["README.md", "src/main.ts"],
      relatedPages: [],
      content: "old overview",
    },
    {
      id: "settings",
      file: "02-settings.md",
      title: "Settings",
      description: "Settings",
      section: null,
      importance: "medium",
      relevantFiles: ["src/settings.ts"],
      relatedPages: [],
      content: "old settings",
    },
  ];
  return {
    meta: {
      version: 1,
      projectPath: "D:\\repos\\wiki-auto-update",
      generatedAt: "2026-08-23T00:00:00Z",
      headSha: "old-head",
      model: "test-model",
      language: "zh-CN",
      status: "completed",
      outline: pages,
      generator: "builtin",
    },
    pages,
    stale: true,
  };
}

describe("wiki store automatic update", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
    vi.mocked(saveWikiMeta).mockResolvedValue(undefined);
    vi.mocked(regenerateWikiPage).mockResolvedValue(undefined);
    vi.mocked(createWikiKernel).mockResolvedValue({
      backendId: "builtin",
      concurrency: 1,
      model: "test-model",
      generateOutline: vi.fn(),
      generatePage: vi.fn(),
      dispose: vi.fn().mockResolvedValue(undefined),
    } satisfies WikiGenKernel);
  });

  it("HEAD 变化后立即按 relevantFiles 重生成命中的页面", async () => {
    const data = createWikiData();
    vi.mocked(loadWiki).mockResolvedValue(data);
    vi.mocked(wikiChangedFiles).mockResolvedValue({
      files: ["docs/notes.md", "src/main.ts"],
      headSha: "new-head",
    });

    const count = await useWikiStore().autoUpdate(
      { path: data.meta.projectPath, name: "wiki-auto-update" },
      "zh-CN",
    );

    expect(count).toBe(1);
    expect(regenerateWikiPage).toHaveBeenCalledTimes(1);
    expect(regenerateWikiPage).toHaveBeenCalledWith(
      expect.anything(),
      data.meta.projectPath,
      expect.objectContaining({ id: "overview" }),
      "zh-CN",
      expect.any(AbortSignal),
      { changedFiles: ["docs/notes.md", "src/main.ts"] },
    );
    expect(saveWikiMeta).toHaveBeenCalledWith(
      data.meta.projectPath,
      expect.objectContaining({ headSha: "new-head" }),
      "update",
    );
  });

  it("没有 relevantFiles 命中时不创建生成内核，只推进已检查 HEAD", async () => {
    const data = createWikiData();
    vi.mocked(loadWiki).mockResolvedValue(data);
    vi.mocked(wikiChangedFiles).mockResolvedValue({
      files: ["docs/notes.md"],
      headSha: "new-head",
    });

    const count = await useWikiStore().autoUpdate(
      { path: data.meta.projectPath, name: "wiki-auto-update" },
      "zh-CN",
    );

    expect(count).toBe(0);
    expect(createWikiKernel).not.toHaveBeenCalled();
    expect(regenerateWikiPage).not.toHaveBeenCalled();
    expect(saveWikiMeta).toHaveBeenCalledWith(
      data.meta.projectPath,
      expect.objectContaining({ headSha: "new-head", model: "test-model" }),
      "update",
    );
  });
});
