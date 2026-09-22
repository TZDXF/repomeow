import { defineStore } from "pinia";
import { ref } from "vue";
import { onListen, runInTerminal } from "@/lib/tauri";
import {
  TERMINAL_OUTPUT_EVENT,
  TERMINAL_SESSION_CHANGED_EVENT,
  appendCapped,
  getCommandSessionOutput,
  listCommandSessions,
  removeCommandSession,
  restartCommandSession,
  runCommandSession,
  stopCommandSession,
  type RunCommandOptions,
} from "@/lib/terminal";
import { useSettingsStore } from "@/stores/settings";
import type { Project, TerminalSessionInfo } from "@/types";

/** 输出增量回调(TerminalView 注册,按会话 id 过滤后写入 xterm) */
type OutputListener = (id: number, chunk: string) => void;

/** 与后端 list_command_sessions 一致:运行中优先,其余按启动时间倒序 */
function sortSessions(list: TerminalSessionInfo[]) {
  list.sort((a, b) => {
    const ra = a.status === "running" ? 1 : 0;
    const rb = b.status === "running" ? 1 : 0;
    return rb - ra || b.started_at - a.started_at;
  });
}

/**
 * 内嵌终端会话状态:订阅后端 `terminal://` 事件维护会话列表与输出缓存。
 * 会话仅存后端内存(应用退出即清空),前端缓存同生命周期;
 * open / activeId 是每个 webview 窗口自己的 UI 状态,不跨窗口同步。
 */
export const useTerminalStore = defineStore("terminal", () => {
  const settings = useSettingsStore();

  /** 全部会话(排序与后端一致) */
  const sessions = ref<TerminalSessionInfo[]>([]);
  /** 终端面板是否展开 */
  const open = ref(false);
  /** 当前选中的会话 id */
  const activeId = ref<number | null>(null);

  /** 每会话输出缓存:事件流自订阅起完整;订阅前已存在的会话在 init 时回填 */
  const buffers = new Map<number, string>();
  /** 各会话最近一次 started_at:restart 后该值变化,据此清空前端缓存 */
  const startedAts = new Map<number, number>();
  const outputListeners = new Set<OutputListener>();

  let initPromise: Promise<void> | null = null;

  /** 订阅事件并回填存量会话;与 settings.init 同模式,并发触发共享同一 Promise */
  function init(): Promise<void> {
    initPromise ??= doInit();
    return initPromise;
  }

  async function doInit() {
    await onListen<TerminalSessionInfo>(TERMINAL_SESSION_CHANGED_EVENT, onSessionChanged);
    await onListen<{ id: number; data: string }>(TERMINAL_OUTPUT_EVENT, (p) =>
      appendOutput(p.id, p.data),
    );
    // 订阅前已存在的会话(如托盘弹窗先行启动):回填元数据与输出缓存
    const existing = await listCommandSessions();
    sessions.value = existing;
    await Promise.all(
      existing.map(async (s) => {
        startedAts.set(s.id, s.started_at);
        // 输出事件可能先于回填到达(append 已建缓存),仅补缺,不覆盖
        if (!buffers.has(s.id)) {
          buffers.set(s.id, await getCommandSessionOutput(s.id).catch(() => ""));
        }
      }),
    );
  }

  function onSessionChanged(info: TerminalSessionInfo) {
    const knownStarted = startedAts.get(info.id);
    // 重启:后端缓冲已清空且 started_at 更新,前端缓存同步清空
    if (knownStarted !== undefined && info.started_at > knownStarted) {
      buffers.set(info.id, "");
    }
    startedAts.set(info.id, info.started_at);
    const idx = sessions.value.findIndex((s) => s.id === info.id);
    if (idx >= 0) {
      sessions.value.splice(idx, 1, info);
    } else {
      sessions.value.unshift(info);
    }
    sortSessions(sessions.value);
  }

  function appendOutput(id: number, chunk: string) {
    if (!chunk) return;
    buffers.set(id, appendCapped(buffers.get(id) ?? "", chunk));
    for (const l of outputListeners) l(id, chunk);
  }

  /**
   * 统一执行入口:内嵌终端开启时在应用内执行并展开面板,关闭时弹系统终端新窗口。
   * 返回实际执行方式,便于调用方做差异化处理(如延迟刷新状态)。
   */
  async function run(
    project: Project,
    command: string,
    opts: RunCommandOptions = {},
  ): Promise<"embedded" | "system"> {
    await init();
    if (!settings.embeddedTerminal) {
      await runInTerminal(project, command, opts.cwd, opts.javaHome);
      return "system";
    }
    const info = await runCommandSession(project, command, opts);
    activeId.value = info.id;
    open.value = true;
    return "embedded";
  }

  /** 停止会话(整棵树);状态翻转由 waiter 的 session-changed 事件带回 */
  async function stop(id: number) {
    await stopCommandSession(id);
  }

  /** 重启会话:后端会再广播 session-changed,这里同步一次让 UI 立即翻转 */
  async function restart(id: number) {
    onSessionChanged(await restartCommandSession(id));
  }

  /** 移除会话(运行中的后端先停止);选中态顺延到剩余首个 */
  async function remove(id: number) {
    await removeCommandSession(id);
    sessions.value = sessions.value.filter((s) => s.id !== id);
    buffers.delete(id);
    startedAts.delete(id);
    if (activeId.value === id) {
      activeId.value = sessions.value[0]?.id ?? null;
    }
  }

  /** 清除已结束会话;传 projectId 时仅清该项目的 */
  async function clearFinished(projectId?: number) {
    const finished = sessions.value.filter(
      (s) => s.status !== "running" && (projectId === undefined || s.project_id === projectId),
    );
    for (const s of finished) {
      await remove(s.id).catch(() => {});
    }
  }

  /** 会话输出缓存(面板打开/切换会话时一次性渲染,此后走 onOutput 增量) */
  function outputOf(id: number): string {
    return buffers.get(id) ?? "";
  }

  /** 注册输出增量回调,返回注销函数 */
  function onOutput(listener: OutputListener): () => void {
    outputListeners.add(listener);
    return () => outputListeners.delete(listener);
  }

  return {
    sessions,
    open,
    activeId,
    init,
    run,
    stop,
    restart,
    remove,
    clearFinished,
    outputOf,
    onOutput,
  };
});
