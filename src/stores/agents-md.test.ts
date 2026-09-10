import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useAgentsMdStore } from "@/stores/agents-md";
import type { Project } from "@/types";

const { generateAgentsMdMock, toastSuccess, toastError } = vi.hoisted(() => ({
  generateAgentsMdMock: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
}));

vi.mock("@/lib/ai", () => ({ generateAgentsMd: generateAgentsMdMock }));
vi.mock("vue-sonner", () => ({
  toast: { success: toastSuccess, error: toastError },
}));

const project = { id: 7, name: "喵库", path: "D:\\code\\repomeow" } as Project;

describe("agents-md store", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setActivePinia(createPinia());
  });

  it("生成挂在 store 上:进行态可查询并镜像为后台任务,成功后 toast 并收敛状态", async () => {
    generateAgentsMdMock.mockResolvedValue({ claudeAction: "aligned" });
    const store = useAgentsMdStore();
    const run = store.generate(project, "zh-CN", { model: "deepseek/deepseek-v4-pro" });
    // 状态同步进入 running(不等待 IPC 返回),后台任务中心立即可见
    expect(store.isGenerating(project.path)).toBe(true);
    expect(store.backgroundTasks).toEqual([
      { id: expect.stringContaining("agents-md:"), projectId: 7, projectName: "喵库" },
    ]);
    await run;
    expect(store.isGenerating(project.path)).toBe(false);
    expect(store.generationFor(project.path)?.status).toBe("done");
    expect(store.generationFor(project.path)?.finishedAt).not.toBeNull();
    expect(store.backgroundTasks).toEqual([]);
    expect(toastSuccess).toHaveBeenCalledOnce();
    // 模型/思考强度与取消信号一并透传
    const [, , options] = generateAgentsMdMock.mock.calls[0]!;
    expect(options.model).toBe("deepseek/deepseek-v4-pro");
    expect(options.signal).toBeInstanceOf(AbortSignal);
  });

  it("同一项目重复启动被忽略", async () => {
    generateAgentsMdMock.mockReturnValue(new Promise(() => {}));
    const store = useAgentsMdStore();
    void store.generate(project, "zh-CN", {});
    void store.generate(project, "zh-CN", {});
    expect(generateAgentsMdMock).toHaveBeenCalledOnce();
  });

  it("取消后静默收场(无 toast),状态为 cancelled", async () => {
    generateAgentsMdMock.mockResolvedValue(null);
    const store = useAgentsMdStore();
    await store.generate(project, "zh-CN", {});
    expect(store.generationFor(project.path)?.status).toBe("cancelled");
    expect(toastSuccess).not.toHaveBeenCalled();
    expect(toastError).not.toHaveBeenCalled();
  });

  it("失败 toast 并记录 failed", async () => {
    generateAgentsMdMock.mockRejectedValue(new Error("boom"));
    const store = useAgentsMdStore();
    await store.generate(project, "zh-CN", {});
    expect(store.generationFor(project.path)?.status).toBe("failed");
    expect(toastError).toHaveBeenCalledOnce();
  });
});
