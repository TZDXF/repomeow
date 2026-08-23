import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { i18n } from "@/i18n";
import { regenerateWikiPage, updateWiki } from "@/lib/wiki-generator";
import { loadWiki } from "@/lib/wiki";
import { toFriendlyWikiGenerationError, useWikiStore } from "@/stores/wiki";

const generationHarness = vi.hoisted(() => {
  const finishes = new Map<string, () => void>();
  const errors = new Map<string, Error>();
  return { errors, finishes };
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
      "AI 返回的大纲格式不完整。请重试生成；如果多次失败，请更换模型或生成后端。",
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
    vi.mocked(updateWiki).mockResolvedValue(1);
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
    vi.mocked(updateWiki).mockResolvedValue(0);
    const project = { path: "D:\\repos\\wiki-auto-update", name: "wiki-auto-update" };

    const count = await useWikiStore().autoUpdate(project, "zh-CN");

    expect(count).toBe(0);
    expect(regenerateWikiPage).not.toHaveBeenCalled();
    expect(updateWiki).toHaveBeenCalledOnce();
  });
});
