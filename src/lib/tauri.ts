import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Project } from "@/types";
import { i18n } from "@/i18n";

type SerializedAppError = {
  code?: unknown;
  message?: unknown;
};

type WrappedCommandError = Error & {
  /** Rust 后端错误码(供 catch 站点做条件分支,如推送被拒绝时给出快捷操作) */
  code?: string;
  cause?: unknown;
};

/**
 * 将 Tauri 拒绝对象翻译为本地化文本。
 * - 命中 `errors.<code>` i18n key → 返回本地化文本
 * - 未命中但有 `code` → 返回 `code` + `message`(技术上下文)
 * - 有 `message` → 返回 `message`
 * - 兜底 → "未知错误"
 */
function translateCommandError(error: unknown): string {
  if (error instanceof Error) {
    return error.message || String(error);
  }

  if (error && typeof error === "object") {
    const serialized = error as SerializedAppError;
    const message = typeof serialized.message === "string" ? serialized.message : "";
    const code = typeof serialized.code === "string" ? serialized.code : "";
    if (code && i18n.global.te(`errors.${code}`)) {
      return i18n.global.t(`errors.${code}`);
    }
    // 未命中 i18n:有 message 用 message,否则兜底("code:message" 形式对用户无意义)
    if (message) {
      return message;
    }
    if (code) {
      return code;
    }
  }

  return "未知错误";
}

/** 测试入口,行为与命令错误翻译一致 */
export function translateCommandErrorForTest(error: unknown): string {
  return translateCommandError(error);
}

/** 调用 Rust 命令,参数 key 用 camelCase(Tauri 自动映射 snake_case) */
export async function cmd<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(name, args);
  } catch (error) {
    const wrapped = new Error(translateCommandError(error)) as WrappedCommandError;
    wrapped.cause = error;
    if (error && typeof error === "object") {
      const code = (error as SerializedAppError).code;
      if (typeof code === "string" && code) {
        wrapped.code = code;
      }
    }
    throw wrapped;
  }
}

/** 监听后端事件 */
export function onListen<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => handler(e.payload));
}

/** 在系统终端里执行命令(新窗口,跑完不关);cwd 缺省为项目根目录 */
export function runInTerminal(project: Project, command: string, cwd?: string): Promise<unknown> {
  return cmd("run_in_terminal", {
    path: project.path,
    projectName: project.name,
    command,
    ...(cwd ? { cwd } : {}),
  });
}
