// ── AI 面板(scan_project_ai_assets 只读扫描) ─────────────────

/** 项目内检测到的一个 AI 指令/规则/设置文件 */
export interface AiAssetItem {
  /** 仓库相对路径('/' 分隔) */
  path: string;
  /** instruction(行为指令)/ rule(规则)/ setting(配置) */
  kind: "instruction" | "rule" | "setting";
  /** 归属的 agent id */
  agents: string[];
}

/** MCP 配置文件方言,决定服务器定义的字段映射与文件格式 */
export type McpDialect = "claude" | "codex" | "gemini" | "opencode";

/** 项目内 MCP 配置文件中的一个服务器条目 */
export interface McpServerEntry {
  name: string;
  /** 原始服务器定义(各方言字段不同,claude: command/args/env;opencode: command 数组等) */
  config: Record<string, unknown>;
}

/** 项目内的一个 MCP 配置文件及其声明的服务器 */
export interface ProjectMcpFile {
  path: string;
  dialect: McpDialect;
  /** 该文件归属的 agent id */
  agents: string[];
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

/** scan_project_ai_assets 的聚合结果(详情页 AI 面板数据源) */
export interface ProjectAiAssets {
  files: AiAssetItem[];
  mcp: ProjectMcpFile[];
  skills: ProjectSkill[];
}
