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
 * 通用兜底错误码:i18n 文案仅笼统的"X 失败",诊断价值几乎全靠后端附带的原始
 * stderr / 错误细节(message)。命中这些码时把 message 追加展示,避免用户只看到
 * 一句无信息的"git 命令失败"。特定码(如 git_repo_not_found)后端 message 为空,
 * 不在此列,仅展示友好的 i18n 文案。
 */
const GENERIC_DETAIL_CODES = new Set([
  "git_command_failed",
  "git_push_failed",
  "git_pull_failed",
  "git_log_failed",
  "git_clone_failed",
  "git_task_failed",
  "git_noise_fallback",
  "search_invalid_glob",
  // Docker:i18n 文案只有一句"操作失败",不附带 message 就丢了 stderr 诊断信息
  "docker_action_failed",
  "docker_save_failed",
  "docker_compose_parse_failed",
  "docker_exec_failed",
  "docker_task_failed",
  // JDK 在线安装:文案只有一句"安装失败",message 里的 URL/HTTP 状态/目标路径是排障关键
  "jdk_install_failed",
  // 工具链操作:message 里的工具/操作名或非法版本值是定位问题的关键
  "toolchain_op_unsupported",
  "toolchain_version_invalid",
]);

/**
 * 将 Tauri 拒绝对象翻译为本地化文本。
 * - 命中 `errors.<code>` i18n key → 返回本地化文本(通用兜底码额外追加原始 message)
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
      const base = i18n.global.t(`errors.${code}`);
      // 通用兜底码:i18n 文案过笼统,需把后端携带的原始细节一并展示才有诊断价值
      return GENERIC_DETAIL_CODES.has(code) && message ? `${base}\n${message}` : base;
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

/**
 * 在系统终端里执行命令(新窗口,跑完不关);cwd 缺省为项目根目录。
 * javaHome 非空时后端在命令前注入 JAVA_HOME(Spring Boot 运行用)。
 */
export function runInTerminal(
  project: Project,
  command: string,
  cwd?: string,
  javaHome?: string,
): Promise<unknown> {
  return cmd("run_in_terminal", {
    path: project.path,
    projectName: project.name,
    command,
    ...(cwd ? { cwd } : {}),
    ...(javaHome ? { javaHome } : {}),
  });
}
