import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useWikiStore } from "@/stores/wiki";

const generationHarness = vi.hoisted(() => {
  const finishes = new Map<string, () => void>();
  return { finishes };
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
    wikiAutoUpdateThreshold: 10,
  }),
}));

describe("wiki store generation concurrency", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    generationHarness.finishes.clear();
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
});
