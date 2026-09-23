/** 内嵌终端会话状态(与 Rust TerminalSessionStatus 的 snake_case 序列化一致) */
export type TerminalSessionStatus = "running" | "exited" | "stopped" | "spawn_failed";

/**
 * 内嵌终端会话元数据(与 Rust TerminalSessionInfo 字段一致,snake_case)。
 * 输出走 terminal://output 事件与 get_command_session_output 回放,不随元数据传输。
 */
export interface TerminalSessionInfo {
  id: number;
  project_id: number;
  project_name: string;
  /** 展示名(npm script 名 / 自定义命令名 / docker 操作),缺省回退 command */
  label: string;
  /** 来源分类:npm / docker / custom / java / shell */
  kind: string;
  command: string;
  /** 主动创建的逐行输入 Shell;一次性命令会话为 false */
  interactive: boolean;
  /** 实际工作目录(绝对路径) */
  cwd: string;
  status: TerminalSessionStatus;
  exit_code: number | null;
  /** 启动时间(Unix 秒) */
  started_at: number;
  finished_at: number | null;
}
