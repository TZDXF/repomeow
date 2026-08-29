import { computed, reactive, ref } from "vue";
import { defineStore } from "pinia";
import type { BackgroundTaskProgressPayload } from "@/types";

export type BackgroundTaskKind = "report" | "wiki" | "conflict";

export type BackgroundTaskTarget =
  | { kind: "wiki"; projectId: number }
  | { kind: "project"; projectId: number };

export interface BackgroundTaskItem {
  id: string;
  source: string;
  kind: BackgroundTaskKind;
  label: string;
  completed: number;
  total: number;
  status: "running" | "finished";
  startedAt: number;
  updatedAt: number;
  finishedAt: number | null;
  target?: BackgroundTaskTarget;
}

export interface FrontendBackgroundTask {
  id: string;
  kind: BackgroundTaskKind;
  label: string;
  completed: number;
  total: number;
  target?: BackgroundTaskTarget;
}

const HISTORY_STORAGE_KEY = "repomeow.background-task-history.v1";
export const BACKGROUND_TASK_HISTORY_RETENTION_MS = 24 * 60 * 60 * 1000;
export const BACKGROUND_TASK_HISTORY_LIMIT_PER_KIND = 10;

function storage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function isTaskKind(value: unknown): value is BackgroundTaskKind {
  return value === "report" || value === "wiki" || value === "conflict";
}

function isTaskTarget(value: unknown): value is BackgroundTaskTarget {
  if (!value || typeof value !== "object") {
    return false;
  }
  const target = value as Partial<BackgroundTaskTarget>;
  return (
    (target.kind === "wiki" || target.kind === "project") && typeof target.projectId === "number"
  );
}

function isStoredTask(value: unknown): value is BackgroundTaskItem {
  if (!value || typeof value !== "object") {
    return false;
  }
  const task = value as Partial<BackgroundTaskItem>;
  return (
    typeof task.id === "string" &&
    typeof task.source === "string" &&
    isTaskKind(task.kind) &&
    typeof task.label === "string" &&
    typeof task.completed === "number" &&
    typeof task.total === "number" &&
    task.status === "finished" &&
    typeof task.startedAt === "number" &&
    typeof task.updatedAt === "number" &&
    typeof task.finishedAt === "number" &&
    (task.target === undefined || isTaskTarget(task.target))
  );
}

function pruneHistory(items: BackgroundTaskItem[], now = Date.now()): BackgroundTaskItem[] {
  const counts: Partial<Record<BackgroundTaskKind, number>> = {};
  return items
    .filter(
      (task) =>
        task.status === "finished" &&
        task.finishedAt !== null &&
        task.finishedAt >= now - BACKGROUND_TASK_HISTORY_RETENTION_MS,
    )
    .sort((a, b) => (b.finishedAt ?? 0) - (a.finishedAt ?? 0))
    .filter((task) => {
      const count = counts[task.kind] ?? 0;
      if (count >= BACKGROUND_TASK_HISTORY_LIMIT_PER_KIND) {
        return false;
      }
      counts[task.kind] = count + 1;
      return true;
    });
}

function loadHistory(): BackgroundTaskItem[] {
  const target = storage();
  const value = target?.getItem(HISTORY_STORAGE_KEY);
  if (!value) {
    return [];
  }
  try {
    const parsed: unknown = JSON.parse(value);
    const history = pruneHistory(Array.isArray(parsed) ? parsed.filter(isStoredTask) : []);
    // 兼容旧版本：Git 检查不再属于可记录的后台任务，读取时同步清掉历史记录。
    try {
      target?.setItem(HISTORY_STORAGE_KEY, JSON.stringify(history));
    } catch {
      // 清理持久化数据失败不影响内存中的有效历史。
    }
    return history;
  } catch {
    return [];
  }
}

/**
 * 标题栏任务中心：活动任务只保存在内存中，完成记录短期写入 localStorage。
 * Rust 任务走 applyProgress；前端 Store 任务由 syncFrontendTasks 镜像进来。
 */
export const useBackgroundTasksStore = defineStore("backgroundTasks", () => {
  const tasksById = reactive<Record<string, BackgroundTaskItem>>({});
  const history = ref<BackgroundTaskItem[]>(loadHistory());

  const tasks = computed(() => Object.values(tasksById).sort((a, b) => a.startedAt - b.startedAt));

  function persistHistory() {
    const target = storage();
    if (!target) {
      return;
    }
    try {
      target.setItem(HISTORY_STORAGE_KEY, JSON.stringify(history.value));
    } catch {
      // localStorage 被禁用或空间不足时仅丢失历史，不影响任务执行。
    }
  }

  function upsert(
    input: {
      id: string;
      source: string;
      task: Omit<FrontendBackgroundTask, "id">;
    },
    now = Date.now(),
  ) {
    const { id, source, task } = input;
    const previous = tasksById[id];
    tasksById[id] = {
      id,
      source,
      kind: task.kind,
      label: task.label,
      completed: task.completed,
      total: task.total,
      status: "running",
      startedAt: previous?.startedAt ?? now,
      updatedAt: now,
      finishedAt: null,
      target: task.target,
    };
  }

  function finish(id: string, now = Date.now()) {
    const task = tasksById[id];
    if (!task) {
      return;
    }
    delete tasksById[id];
    history.value = pruneHistory([
      {
        ...task,
        completed: task.total > 0 ? task.total : task.completed,
        status: "finished",
        updatedAt: now,
        finishedAt: now,
      },
      ...history.value,
    ]);
    persistHistory();
  }

  function applyProgress(payload: BackgroundTaskProgressPayload) {
    if (!isTaskKind(payload.kind)) {
      return;
    }
    const id = `backend:${payload.task_id}`;
    if (payload.status === "finished") {
      finish(id);
      return;
    }
    upsert({
      id,
      source: "backend",
      task: {
        kind: payload.kind,
        label: payload.label,
        completed: payload.completed,
        total: payload.total,
        target:
          payload.kind === "conflict" && typeof payload.project_id === "number"
            ? { kind: "project", projectId: payload.project_id }
            : undefined,
      },
    });
  }

  /** 以命名空间全量同步一组前端任务；上次存在、本次消失的任务自动进入历史。 */
  function syncFrontendTasks(namespace: string, nextTasks: FrontendBackgroundTask[]) {
    const source = `frontend:${namespace}`;
    const nextIds = new Set(nextTasks.map((task) => `${source}:${task.id}`));
    for (const task of Object.values(tasksById)) {
      if (task.source === source && !nextIds.has(task.id)) {
        finish(task.id);
      }
    }
    for (const task of nextTasks) {
      upsert({ id: `${source}:${task.id}`, source, task });
    }
  }

  function clearHistory() {
    history.value = [];
    persistHistory();
  }

  function refreshHistory() {
    const next = pruneHistory(history.value);
    if (next.length !== history.value.length) {
      history.value = next;
      persistHistory();
    }
  }

  return { tasks, history, applyProgress, syncFrontendTasks, clearHistory, refreshHistory };
});
