import type { CustomCommand } from "./project";
import type { HiddenItem } from "./open";

export interface ProjectOverview {
  hidden_items: HiddenItem[];
  custom_commands: CustomCommand[];
}

/** 可标记为「常用」的命令类型 */
export type PinKind =
  | "packageScript"
  | "composeFile"
  | "composeService"
  | "customCommand"
  | "javaBuild";

/**
 * 一条被标记为「常用」的命令,在托盘弹窗项目列表中可直接执行
 * target_key: packageScript = "<dir>\n<name>";composeFile = 文件相对路径;
 *             composeService = "<file>\n<service>";customCommand = 命令 id
 * command: npm/自定义为完整命令;compose 类为基础前缀 `docker compose -f "..."`,动作在执行时拼接
 */
export interface PinnedCommand {
  id: number;
  project_id: number;
  kind: PinKind;
  target_key: string;
  label: string;
  command: string;
  /** 可选工作目录:相对项目根(monorepo 子包),执行时拼接 project.path,迁移目录后仍可用 */
  cwd: string | null;
  /** 自定义命令的图标名(后端 list 时实时 JOIN custom_commands,其他 kind 为 null) */
  icon: string | null;
  created_at: number;
}

/** 报告类型:日报(单日) | 周报(日期范围) */
