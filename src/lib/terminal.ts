import { cmd } from "@/lib/tauri";
import type { TerminalKind } from "@/stores/settings";
import type { Project, TerminalSessionInfo } from "@/types";

export interface TerminalCapabilities {
  isWindows: boolean;
  windowsTerminal: boolean;
  shells: Record<TerminalKind, boolean>;
}

/** 探测 Windows 终端宿主与各 Shell 的可用性；每次进入设置页重新读取当前环境。 */
export function getTerminalCapabilities(): Promise<TerminalCapabilities | null> {
  return cmd<TerminalCapabilities>("detect_terminal_capabilities").catch(() => null);
}

// ── 内嵌终端会话(run_command_session 系列) ──────────────────────────

/** 会话元数据变更事件,payload 为 TerminalSessionInfo */
export const TERMINAL_SESSION_CHANGED_EVENT = "terminal://session-changed";
/** 会话输出增量事件,payload 为 { id, data } */
export const TERMINAL_OUTPUT_EVENT = "terminal://output";

/** 前端输出缓存上限(与后端 MAX_OUTPUT_CHARS 一致,超出丢弃最旧部分) */
export const TERMINAL_MAX_OUTPUT_CHARS = 400_000;

/** 追加输出并封顶(超出丢弃最旧部分),返回新的缓冲内容 */
export function appendCapped(
  prev: string,
  chunk: string,
  max: number = TERMINAL_MAX_OUTPUT_CHARS,
): string {
  const next = prev + chunk;
  return next.length > max ? next.slice(next.length - max) : next;
}

/** 会话状态对应的圆点着色(TerminalPanel / TerminalSessionsMenu 共用):exited 按退出码区分 */
export function terminalStatusDotClass(
  s: Pick<TerminalSessionInfo, "status" | "exit_code">,
): string {
  if (s.status === "running") return "bg-emerald-500 animate-pulse";
  if (s.status === "stopped") return "bg-amber-500";
  if (s.status === "spawn_failed") return "bg-red-500";
  return s.exit_code === 0 ? "bg-muted-foreground" : "bg-red-500";
}
/** 内嵌终端执行的可选参数(cwd 缺省为项目根;kind/label 仅用于前端分组展示) */
export interface RunCommandOptions {
  cwd?: string;
  /** 非空时以进程环境注入 JAVA_HOME(Spring Boot 运行用) */
  javaHome?: string;
  kind?: string;
  label?: string;
}

/** 在内嵌终端启动新会话,返回会话元数据(输出经事件流式推送) */
export function runCommandSession(
  project: Project,
  command: string,
  opts: RunCommandOptions = {},
): Promise<TerminalSessionInfo> {
  return cmd<TerminalSessionInfo>("run_command_session", {
    projectId: project.id,
    projectName: project.name,
    path: project.path,
    command,
    ...(opts.cwd ? { cwd: opts.cwd } : {}),
    ...(opts.javaHome ? { javaHome: opts.javaHome } : {}),
    ...(opts.kind ? { kind: opts.kind } : {}),
    ...(opts.label ? { label: opts.label } : {}),
  });
}

/** 在项目当前工作目录新建可输入命令的 Shell 会话 */
export function createShellSession(project: Project): Promise<TerminalSessionInfo> {
  return cmd<TerminalSessionInfo>("create_shell_session", {
    projectId: project.id,
    projectName: project.name,
    path: project.path,
  });
}

/** 全部会话:运行中优先,其余按启动时间倒序 */
export function listCommandSessions(): Promise<TerminalSessionInfo[]> {
  return cmd<TerminalSessionInfo[]>("list_command_sessions");
}

/** 回放会话输出缓冲(打开面板/切换会话时一次性拉取,此后走事件增量) */
export function getCommandSessionOutput(id: number): Promise<string> {
  return cmd<string>("get_command_session_output", { id });
}

/** 停止会话(Windows 整棵树 taskkill);进程退出后状态归为 stopped */
export function stopCommandSession(id: number): Promise<void> {
  return cmd<void>("stop_command_session", { id });
}

/** 重启会话:id 不变,输出缓冲清空,以原参数重新拉起 */
export function restartCommandSession(id: number): Promise<TerminalSessionInfo> {
  return cmd<TerminalSessionInfo>("restart_command_session", { id });
}

/** 向会话 stdin 写入(终端键盘输入);会话已结束时静默忽略 */
export function writeCommandSession(id: number, data: string): Promise<void> {
  return cmd<void>("write_command_session", { id, data });
}

/** 从注册表移除会话(运行中的先停止) */
export function removeCommandSession(id: number): Promise<void> {
  return cmd<void>("remove_command_session", { id });
}
