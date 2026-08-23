import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { useBackgroundTasksStore } from "@/stores/background-tasks";

describe("background tasks store", () => {
  const values = new Map<string, string>();

  beforeEach(() => {
    values.clear();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
      removeItem: (key: string) => values.delete(key),
      clear: () => values.clear(),
      key: (index: number) => [...values.keys()][index] ?? null,
      get length() {
        return values.size;
      },
    } satisfies Storage);
    setActivePinia(createPinia());
  });

  it("按任务 id 更新进度并在结束时移除", () => {
    const store = useBackgroundTasksStore();
    store.applyProgress({
      task_id: "report-1",
      kind: "report",
      label: "日报",
      completed: 0,
      total: 3,
      status: "running",
    });
    store.applyProgress({
      task_id: "report-1",
      kind: "report",
      label: "日报",
      completed: 2,
      total: 3,
      status: "running",
    });

    expect(store.tasks).toEqual([
      expect.objectContaining({ id: "backend:report-1", completed: 2, total: 3 }),
    ]);

    store.applyProgress({
      task_id: "report-1",
      kind: "report",
      label: "日报",
      completed: 3,
      total: 3,
      status: "finished",
    });
    expect(store.tasks).toEqual([]);
    expect(store.history).toEqual([
      expect.objectContaining({ id: "backend:report-1", status: "finished", completed: 3 }),
    ]);

    setActivePinia(createPinia());
    expect(useBackgroundTasksStore().history).toHaveLength(1);
  });

  it("同步前端 Wiki 任务并保留可跳转目标", () => {
    const store = useBackgroundTasksStore();
    store.syncFrontendTasks("title-bar", [
      {
        id: "generate:repo",
        kind: "wiki",
        label: "RepoMeow · 整本生成",
        completed: 1,
        total: 4,
        target: { kind: "wiki", projectId: 7 },
      },
    ]);
    expect(store.tasks[0]).toEqual(
      expect.objectContaining({
        id: "frontend:title-bar:generate:repo",
        target: { kind: "wiki", projectId: 7 },
      }),
    );

    store.syncFrontendTasks("title-bar", []);
    expect(store.tasks).toHaveLength(0);
    expect(store.history[0]?.target).toEqual({ kind: "wiki", projectId: 7 });
  });

  it("每类历史最多保留十条", () => {
    const store = useBackgroundTasksStore();
    for (let index = 0; index < 12; index += 1) {
      store.applyProgress({
        task_id: `report-${index}`,
        kind: "report",
        label: "日报",
        completed: 0,
        total: 1,
        status: "running",
      });
      store.applyProgress({
        task_id: `report-${index}`,
        kind: "report",
        label: "日报",
        completed: 1,
        total: 1,
        status: "finished",
      });
    }
    expect(store.history).toHaveLength(10);
  });

  it("忽略 Git 进度并清理旧版本留下的 Git 历史", () => {
    const key = "repomeow.background-task-history.v1";
    values.set(
      key,
      JSON.stringify([
        {
          id: "backend:git-1",
          source: "backend",
          kind: "git",
          label: "periodic",
          completed: 3,
          total: 3,
          status: "finished",
          startedAt: Date.now() - 1000,
          updatedAt: Date.now(),
          finishedAt: Date.now(),
        },
      ]),
    );

    const store = useBackgroundTasksStore();
    expect(store.history).toEqual([]);
    expect(values.get(key)).toBe("[]");

    store.applyProgress({
      task_id: "git-2",
      kind: "git",
      label: "manual",
      completed: 0,
      total: 3,
      status: "running",
    });
    expect(store.tasks).toEqual([]);
  });
});
