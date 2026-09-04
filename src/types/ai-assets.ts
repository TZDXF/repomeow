// ── AI 面板(scan_project_ai_assets / cc-switch 导入) ─────────────────

/** 项目内检测到的一个 AI 指令/规则/设置文件 */
export interface AiAssetItem {
  /** 仓库相对路径('/' 分隔) */
  path: string;
  /** instruction(行为指令)/ rule(规则)/ setting(配置) */
  kind: "instruction" | "rule" | "setting";
  /** 归属的 agent id */
  agents: string[];
}

/** 项目内 MCP 配置文件中的一个服务器条目 */
export interface McpServerEntry {
  name: string;
  /** 原始服务器定义(stdio: command/args/env;远程: url/type/headers 等) */
  config: Record<string, unknown>;
}

/** 项目内的一个 MCP 配置文件及其声明的服务器 */
export interface ProjectMcpFile {
  path: string;
  /** 服务器对象在文件里的键名(mcpServers / servers),写回时原样传回 */
  serversKey: string;
  /** 服务器条目按名称排序;文件解析失败为空列表(文件仍列出) */
  servers: McpServerEntry[];
}

/** 项目 skills 目录(.claude/skills、.agents/skills、.zcode/skills)下的一个技能 */
export interface ProjectSkill {
  /** 技能目录的仓库相对路径,如 ".claude/skills/foo" */
  dir: string;
  name: string;
  description: string;
  /** frontmatter description 按固定 o200k_base 编码器统计的 token 数 */
  descriptionTokenCount: number;
  /** 完整 SKILL.md 按固定 o200k_base 编码器统计的 token 数 */
  tokenCount: number;
}

/** 一个 agent 工具的本机安装状态 + 本项目配置命中情况 */
export interface ProjectAgentStatus {
  id: string;
  name: string;
  installed: boolean;
  /** 本项目内检测到的、该 agent 会读取的配置路径 */
  configs: string[];
}

/** scan_project_ai_assets 的聚合结果(详情页 AI 面板数据源) */
export interface ProjectAiAssets {
  files: AiAssetItem[];
  mcp: ProjectMcpFile[];
  skills: ProjectSkill[];
  agents: ProjectAgentStatus[];
}

/** cc-switch 管理的一个技能(~/.cc-switch/skills/<directory>/) */
export interface CcSwitchSkill {
  id: string;
  name: string;
  description: string;
  /** skills/ 下的子目录名(导出到项目时的目标目录名) */
  directory: string;
  enabledApps: string[];
}

/** cc-switch 管理的一个 MCP 服务器 */
export interface CcSwitchMcpServer {
  id: string;
  name: string;
  description: string;
  tags: string[];
  /** 原始服务器定义(stdio/sse/http 等),导出时按 name 键写入项目 .mcp.json */
  serverConfig: Record<string, unknown>;
  enabledApps: string[];
}

/** ai_cc_switch_assets 的结果;found=false 表示本机没有 ~/.cc-switch */
export interface CcSwitchAssets {
  found: boolean;
  skills: CcSwitchSkill[];
  mcpServers: CcSwitchMcpServer[];
}
