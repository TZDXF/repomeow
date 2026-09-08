import { cmd, onListen } from "@/lib/tauri";

/**
 * 设置页全局资源库的唯一 IPC 桥接层。
 * 数据由后端固定存放在 ~/.repomeow/resource-library，前端不参与路径计算。
 */

export interface ResourceSkillGroup {
  id: string;
  name: string;
  /** 分组描述;空串 = 未填写,仅前端展示 */
  description: string;
  color?: string;
  sortOrder: number;
}

export interface ResourceSkill {
  id: string;
  name: string;
  description: string;
  directory: string;
  /** 来自 skills.sh 的来源元数据；手动创建 Skill 不带此字段。 */
  marketplace?: ResourceSkillMarketplace;
  groupIds: string[];
  sortOrder: number;
  updatedAt?: number;
}

/** 市场技能来源与安装基线;installedSha 未知时检查更新回退内容比对 */
export interface ResourceSkillMarketplace {
  id: string;
  source: string;
  url: string;
  /** 技能在 GitHub 仓库内的目录(仓库相对路径),空串 = 仓库根 */
  repoDir?: string;
  /** 安装/最近更新时的 GitHub commit sha */
  installedSha?: string | null;
  installedAt?: number | null;
}

export interface ResourceSkillList {
  groups: ResourceSkillGroup[];
  skills: ResourceSkill[];
}

export interface ResourceSkillInput {
  name: string;
  description?: string;
  groupIds: string[];
  body?: string;
}

type BackendSkillGroup = Omit<ResourceSkillGroup, "sortOrder"> & { sort: number };
type BackendSkillList = { groups: BackendSkillGroup[]; skills: ResourceSkill[] };

function mapGroup(group: BackendSkillGroup): ResourceSkillGroup {
  return { ...group, sortOrder: group.sort };
}

export async function listResourceSkills(): Promise<ResourceSkillList> {
  const data = await cmd<BackendSkillList>("rl_skill_list");
  return {
    groups: data.groups.map(mapGroup).sort((a, b) => a.sortOrder - b.sortOrder),
    skills: [...data.skills].sort((a, b) => a.sortOrder - b.sortOrder),
  };
}

export function updateResourceSkill(id: string, input: ResourceSkillInput): Promise<ResourceSkill> {
  return cmd<ResourceSkill>("rl_skill_update", {
    id,
    name: input.name,
    description: input.description ?? "",
    groupIds: input.groupIds,
    body: input.body,
  });
}

/** 仅更新技能的分组归属(name/description 缺省,后端不改写 SKILL.md frontmatter) */
export function updateResourceSkillGroups(id: string, groupIds: string[]): Promise<ResourceSkill> {
  return cmd<ResourceSkill>("rl_skill_update", { id, groupIds });
}

export async function readResourceSkillBody(id: string): Promise<{ body: string }> {
  const result = await cmd<{ content: string }>("rl_skill_body_read", { id });
  return { body: result.content };
}

export function saveResourceSkillBody(id: string, body: string): Promise<void> {
  return cmd<void>("rl_skill_body_write", { id, content: body });
}

// ---------------------------------------------------------------------------
// Skill 预览页:token 统计与安全扫描(Rust 侧 rl_skill_tokens / rl_skill_scan)
// ---------------------------------------------------------------------------

/** 技能目录内单文件的 token 统计(path 为相对技能目录路径,SKILL.md 为正文) */
export interface ResourceSkillTokenFile {
  path: string;
  /** null = 二进制/非 UTF-8 文件,不参与 token 统计 */
  tokens: number | null;
  bytes: number;
}

export interface ResourceSkillTokenReport {
  id: string;
  /** 技能描述(frontmatter 同步值)的 token 占用 */
  descriptionTokens: number;
  /** 全部文本文件 token 合计(二进制文件不计入) */
  totalTokens: number;
  /** SKILL.md 在首位,其余按路径排序;含二进制文件(tokens 为 null) */
  files: ResourceSkillTokenFile[];
  /** 技能内容指纹(描述 + 全部文件路径与字节);扫描结果缓存的有效性判据 */
  hash: string;
}

export function readResourceSkillTokens(id: string): Promise<ResourceSkillTokenReport> {
  return cmd<ResourceSkillTokenReport>("rl_skill_tokens", { id });
}

/** 读取技能目录内单个文件;content 为 null = 二进制/超出上限,无法文本预览 */
export function readResourceSkillFile(
  id: string,
  path: string,
): Promise<{ path: string; content: string | null }> {
  return cmd<{ path: string; content: string | null }>("rl_skill_file_read", { id, path });
}

/** 单条安全发现:静态发现按 ruleId 走 i18n 标题,AI 发现直接用后端文本 */
export interface ResourceSkillScanFinding {
  severity: "critical" | "high" | "medium" | "low" | string;
  category: string;
  ruleId?: string;
  title: string;
  detail: string;
  location: string;
  source: "static" | "llm" | string;
  evidence?: string;
}

export type ResourceSkillScanLevel = "low" | "medium" | "high" | "critical" | string;

export type ResourceSkillScanLlmStatus = "ok" | "skipped" | "canceled" | "failed" | string;

export interface ResourceSkillScanReport {
  skillId: string;
  /** 0-100,技能目录含可执行脚本时评分 ×1.3 */
  score: number;
  level: ResourceSkillScanLevel;
  /** 静态发现(去掉 AI 判定的误报)+ AI 发现 */
  findings: ResourceSkillScanFinding[];
  /** 静态命中总数(含被 AI 过滤的误报) */
  staticCount: number;
  suppressedCount: number;
  filesScanned: number;
  /** ok | skipped(AI 未配置)| canceled | failed */
  llmStatus: ResourceSkillScanLlmStatus;
  llmErrorCode?: string | null;
  llmErrorMessage?: string | null;
  llmSummary?: string | null;
  scannedAt: number;
}

/**
 * 技能安全扫描(参考 SkillSpector 两层管线):静态规则层 + 内置 Agent 语义层。
 * language 决定 AI 发现的输出语言;runId 供 ai_cancel_run 取消语义层;
 * providerId/modelId 为显式选择的扫描模型,缺省走设置页默认模型。
 */
export function scanResourceSkill(
  id: string,
  options: { language: string; runId?: string; providerId?: string; modelId?: string },
): Promise<ResourceSkillScanReport> {
  return cmd<ResourceSkillScanReport>("rl_skill_scan", {
    id,
    language: options.language,
    ...(options.runId ? { runId: options.runId } : {}),
    ...(options.providerId && options.modelId
      ? { providerId: options.providerId, modelId: options.modelId }
      : {}),
  });
}

// ---------------------------------------------------------------------------
// 本地技能目录预览(项目内非托管技能;Rust 侧 skill_dir_*,不经过资源库)
// ---------------------------------------------------------------------------

/** 本地技能目录报告:名称/描述(SKILL.md frontmatter)+ token 统计 + 内容指纹 */
export interface SkillDirReport {
  name: string;
  description: string;
  descriptionTokens: number;
  totalTokens: number;
  files: ResourceSkillTokenFile[];
  hash: string;
}

export function readSkillDirOverview(path: string): Promise<SkillDirReport> {
  return cmd<SkillDirReport>("skill_dir_overview", { path });
}

export function readSkillDirFile(
  path: string,
  file: string,
): Promise<{ path: string; content: string | null }> {
  return cmd<{ path: string; content: string | null }>("skill_dir_file_read", { path, file });
}

/** 本地技能目录安全扫描(与 scanResourceSkill 同一管线) */
export function scanSkillDir(
  path: string,
  options: { language: string; runId?: string; providerId?: string; modelId?: string },
): Promise<ResourceSkillScanReport> {
  return cmd<ResourceSkillScanReport>("skill_dir_scan", {
    path,
    language: options.language,
    ...(options.runId ? { runId: options.runId } : {}),
    ...(options.providerId && options.modelId
      ? { providerId: options.providerId, modelId: options.modelId }
      : {}),
  });
}

export function deleteResourceSkill(id: string): Promise<void> {
  return cmd<void>("rl_skill_delete", { id });
}

export function openResourceSkillDir(id: string): Promise<void> {
  return cmd<void>("rl_skill_open_dir", { id });
}

/** 单条被跳过的导入条目;reason 为后端稳定码,由 i18n 映射文案 */
export interface ResourceSkillImportSkip {
  name: string;
  /** conflict = 与现有技能重名;invalid = 缺 frontmatter name */
  reason: "conflict" | "invalid";
}

/** 批量导入结果:可导入的照常入库,重名/缺 name 的条目跳过 */
export interface ResourceSkillImportOutcome {
  imported: ResourceSkill[];
  skipped: ResourceSkillImportSkip[];
}

export function importResourceSkillFolder(path: string): Promise<ResourceSkillImportOutcome> {
  return cmd<ResourceSkillImportOutcome>("rl_skill_import_folder", { path });
}

export function importResourceSkillArchive(path: string): Promise<ResourceSkillImportOutcome> {
  return cmd<ResourceSkillImportOutcome>("rl_skill_import_archive", { path });
}

export function importResourceSkillUrl(url: string): Promise<ResourceSkillImportOutcome> {
  return cmd<ResourceSkillImportOutcome>("rl_skill_import_url", { url });
}

export async function createResourceSkillGroup(
  name: string,
  color?: string,
  description?: string,
): Promise<ResourceSkillGroup> {
  return mapGroup(
    await cmd<BackendSkillGroup>("rl_skill_group_create", {
      name,
      color,
      description: description ?? "",
    }),
  );
}

export async function updateResourceSkillGroup(
  id: string,
  name: string,
  color?: string,
  description?: string,
): Promise<ResourceSkillGroup> {
  return mapGroup(
    await cmd<BackendSkillGroup>("rl_skill_group_rename", {
      id,
      name,
      color,
      description: description ?? "",
    }),
  );
}

export function deleteResourceSkillGroup(id: string): Promise<void> {
  return cmd<void>("rl_skill_group_delete", { id });
}

export function reorderResourceSkillGroups(orderedIds: string[]): Promise<void> {
  return cmd<void>("rl_skill_group_reorder", { ids: orderedIds });
}

/** 以分组维度整体设定成员技能:skillIds 为该分组完整成员集合,后端差量增删 */
export function setResourceSkillGroupSkills(groupId: string, skillIds: string[]): Promise<void> {
  return cmd<void>("rl_skill_group_set_skills", { groupId, skillIds });
}

/**
 * 技能筛选行里与普通分组并列的「来源」特殊分组:从市场技能的 marketplace.source
 * 自动派生(去重、按名称排序),不可在分组管理对话框中编辑。
 */
export function collectSkillSources(skills: ResourceSkill[]): string[] {
  const sources = new Set<string>();
  for (const skill of skills) {
    if (skill.marketplace?.source) {
      sources.add(skill.marketplace.source);
    }
  }
  return [...sources].sort((a, b) => a.localeCompare(b));
}

/** 关键词(名称/描述,大小写不敏感)与分组、来源过滤;groupId/sourceId 为 null = 不过滤 */
export function filterSkills(
  skills: ResourceSkill[],
  query: string,
  groupId: string | null,
  sourceId: string | null = null,
): ResourceSkill[] {
  const q = query.trim().toLowerCase();
  return skills.filter((skill) => {
    if (groupId !== null && !skill.groupIds.includes(groupId)) return false;
    if (sourceId !== null && skill.marketplace?.source !== sourceId) return false;
    if (!q) return true;
    return skill.name.toLowerCase().includes(q) || skill.description.toLowerCase().includes(q);
  });
}

// ---------------------------------------------------------------------------
// 技能市场(Rust 侧 rl_marketplace_* 命令;后端就绪前前端先编译通过)
// ---------------------------------------------------------------------------

/** 市场浏览模式：全部、趋势或热门；有搜索词时后端自动切换到关键词搜索。 */
export type ResourceMarketplaceMode = "all" | "trending" | "hot";

/** 市场来源仓库。 */
export interface ResourceMarketplaceSource {
  id: string;
  name: string;
  url?: string;
}

/** 市场技能条目; installedSkillId 非空表示已装入本地 Skills 库。 */
export interface ResourceMarketplaceSkill {
  id: string;
  name: string;
  description?: string;
  source: string;
  installs: number;
  url: string;
  installedSkillId?: string;
}

export interface ResourceMarketplaceList {
  sources: ResourceMarketplaceSource[];
  skills: ResourceMarketplaceSkill[];
}

/** 后端目录仅返回 Skill 条目；来源筛选项由前端从当前结果派生。 */
interface BackendMarketplaceList {
  skills: ResourceMarketplaceSkill[];
}

export interface ResourceMarketplaceListOptions {
  mode: ResourceMarketplaceMode;
  query?: string;
  /** 限定来源 id;空值 = 全部来源 */
  source?: string | null;
}

export async function listResourceMarketplaceSkills(
  options: ResourceMarketplaceListOptions,
): Promise<ResourceMarketplaceList> {
  const query = options.query?.trim();
  const source = options.source?.trim();
  const data = await cmd<BackendMarketplaceList>("rl_marketplace_list", {
    mode: options.mode,
    ...(query ? { query } : {}),
    ...(source ? { source } : {}),
  });
  const skills = data.skills ?? [];
  return {
    sources: mergeMarketplaceSources(
      [],
      skills.map((skill) => ({
        id: skill.source,
        name: skill.source,
        url: `https://github.com/${skill.source}`,
      })),
    ),
    skills,
  };
}

/** 安装市场技能到本地 Skills 库,返回新建的本地技能 */
export function installResourceMarketplaceSkill(id: string): Promise<ResourceSkill> {
  return cmd<ResourceSkill>("rl_marketplace_install", { id });
}

/** 单个市场技能的更新检查结果;updateAvailable 为 null 表示无法判断 */
export interface ResourceMarketplaceUpdateStatus {
  skillId: string;
  marketplaceId: string;
  updateAvailable: boolean | null;
  /** 后端稳定错误码(如 resource_library_marketplace_rate_limited),由 i18n 映射 */
  errorCode?: string | null;
}

/** 批量检查全部市场技能的上游更新(GitHub commits sha 对比,旧数据回退内容比对) */
export function checkResourceMarketplaceUpdates(): Promise<ResourceMarketplaceUpdateStatus[]> {
  return cmd<ResourceMarketplaceUpdateStatus[]>("rl_marketplace_check_updates");
}

/** 应用市场更新:重新下载并整体覆盖技能目录(覆盖前自动 git 快照) */
export function updateResourceMarketplaceSkill(id: string): Promise<ResourceSkill> {
  return cmd<ResourceSkill>("rl_marketplace_update_skill", { id });
}

/** 本地按来源与关键词(名称/描述,大小写不敏感)过滤市场条目;sourceId 为 null = 不过滤 */
export function filterMarketplaceSkills(
  skills: ResourceMarketplaceSkill[],
  query: string,
  sourceId: string | null,
): ResourceMarketplaceSkill[] {
  const q = query.trim().toLowerCase();
  return skills.filter((skill) => {
    if (sourceId !== null && skill.source !== sourceId) {
      return false;
    }
    if (!q) {
      return true;
    }
    return (
      skill.name.toLowerCase().includes(q) || skill.description?.toLowerCase().includes(q) === true
    );
  });
}

/** 安装成功后把对应条目标记为已安装(纯函数,返回新数组,不改原数组) */
export function markMarketplaceInstalled(
  skills: ResourceMarketplaceSkill[],
  id: string,
  installedSkillId: string,
): ResourceMarketplaceSkill[] {
  return skills.map((skill) => (skill.id === id ? { ...skill, installedSkillId } : skill));
}

/**
 * 合并跨请求返回的来源清单:search 响应的 sources 可缺席,故按 id 去重累积;
 * 已知来源用新数据就地更新(保留 base 首次出现的顺序),新来源追加在尾部。
 */
export function mergeMarketplaceSources(
  base: ResourceMarketplaceSource[],
  incoming: ResourceMarketplaceSource[],
): ResourceMarketplaceSource[] {
  const merged = new Map(base.map((source) => [source.id, source]));
  for (const source of incoming) {
    const existing = merged.get(source.id);
    merged.set(source.id, existing ? { ...existing, ...source } : source);
  }
  return [...merged.values()];
}

export const RESOURCE_MCP_TRANSPORTS = ["stdio", "http", "sse"] as const;
export type ResourceMcpTransport = (typeof RESOURCE_MCP_TRANSPORTS)[number];

export function isResourceMcpTransport(value: unknown): value is ResourceMcpTransport {
  return (
    typeof value === "string" && RESOURCE_MCP_TRANSPORTS.includes(value as ResourceMcpTransport)
  );
}

export interface ResourceMcpServer {
  id: string;
  name: string;
  description?: string;
  transport: ResourceMcpTransport;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
}

export interface ResourceMcpServerInput {
  name: string;
  description?: string;
  transport: ResourceMcpTransport;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
}

export function listResourceMcpServers(): Promise<ResourceMcpServer[]> {
  return cmd<ResourceMcpServer[]>("rl_mcp_list");
}

export function createResourceMcpServer(
  server: ResourceMcpServerInput,
): Promise<ResourceMcpServer> {
  return cmd<ResourceMcpServer>("rl_mcp_create", { def: server });
}

export function updateResourceMcpServer(
  id: string,
  server: ResourceMcpServerInput,
): Promise<ResourceMcpServer> {
  return cmd<ResourceMcpServer>("rl_mcp_update", { id, def: server });
}

export function deleteResourceMcpServer(id: string): Promise<void> {
  return cmd<void>("rl_mcp_delete", { id });
}

// ---------------------------------------------------------------------------
// MCP JSON 导入(前端解析预览,后端 rl_mcp_import 批量落库)
// ---------------------------------------------------------------------------

/** 单条被跳过的导入条目;reason 为后端稳定码,由 i18n 映射文案 */
export interface ResourceMcpImportSkip {
  name: string;
  /** conflict = 与现有服务器或批内条目重名;invalid = 校验失败 */
  reason: "conflict" | "invalid";
}

/** 批量导入结果:可导入的照常入库,重名/校验失败的条目跳过 */
export interface ResourceMcpImportOutcome {
  imported: ResourceMcpServer[];
  skipped: ResourceMcpImportSkip[];
}

export function importResourceMcpJson(
  defs: ResourceMcpServerInput[],
): Promise<ResourceMcpImportOutcome> {
  return cmd<ResourceMcpImportOutcome>("rl_mcp_import", { defs });
}

/** 解析出的单条待导入条目;name 可为空串(裸单对象 JSON 无名称,导入前须补填) */
export interface ParsedResourceMcpEntry {
  name: string;
  description?: string;
  transport: ResourceMcpTransport;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  headers?: Record<string, string>;
}

/** 解析阶段被跳过的条目;reason 为稳定码:invalid(缺 command/url)/ unsupported(type 不支持) */
export interface ParsedResourceMcpSkip {
  name: string;
  reason: "invalid" | "unsupported";
}

export interface ParsedResourceMcpJson {
  entries: ParsedResourceMcpEntry[];
  skipped: ParsedResourceMcpSkip[];
}

/** JSON 粘贴无法解析时抛出;code 为稳定码,由 i18n 映射文案 */
export class ResourceMcpJsonError extends Error {
  /** invalidJson = 非法 JSON 语法;unrecognized = 结构中不包含任何服务器定义 */
  code: "invalidJson" | "unrecognized";

  constructor(code: "invalidJson" | "unrecognized") {
    super(code);
    this.name = "ResourceMcpJsonError";
    this.code = code;
  }
}

type JsonRecord = Record<string, unknown>;

function isJsonRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 形似服务器定义:含非空字符串 command 或 url 字段 */
function looksLikeServerDef(value: unknown): value is JsonRecord {
  return (
    isJsonRecord(value) &&
    ((typeof value.command === "string" && value.command.trim() !== "") ||
      (typeof value.url === "string" && value.url.trim() !== ""))
  );
}

function jsonStringField(def: JsonRecord, key: string): string | undefined {
  const value = def[key];
  return typeof value === "string" && value.trim() !== "" ? value.trim() : undefined;
}

/** args:字符串视为单参数;数组内标量取字符串化值,其余元素忽略 */
function jsonArgs(value: unknown): string[] | undefined {
  if (typeof value === "string") {
    return value.trim() ? [value.trim()] : undefined;
  }
  if (!Array.isArray(value)) {
    return undefined;
  }
  const args = value
    .map((item) =>
      typeof item === "string" || typeof item === "number" || typeof item === "boolean"
        ? String(item).trim()
        : "",
    )
    .filter(Boolean);
  return args.length ? args : undefined;
}

/** env/headers 键值表:仅保留标量值(字符串化),空键/空值丢弃 */
function jsonMap(value: unknown): Record<string, string> | undefined {
  if (!isJsonRecord(value)) {
    return undefined;
  }
  const map: Record<string, string> = {};
  for (const [key, item] of Object.entries(value)) {
    if (typeof item === "string" || typeof item === "number" || typeof item === "boolean") {
      const text = String(item).trim();
      if (key.trim() && text) {
        map[key.trim()] = text;
      }
    }
  }
  return Object.keys(map).length ? map : undefined;
}

/** type/transport 字段归一:识别各家常见别名;未声明返回 null,声明但未知视作不支持 */
function normalizeJsonTransport(raw: unknown): ResourceMcpTransport | null {
  if (typeof raw !== "string" || raw.trim() === "") {
    return null;
  }
  switch (raw.toLowerCase().replace(/[\s_-]/g, "")) {
    case "stdio":
    case "local":
      return "stdio";
    case "http":
    case "streamablehttp":
    case "remote":
      return "http";
    case "sse":
      return "sse";
    default:
      return null;
  }
}

function collectJsonServerDef(name: string, value: unknown, out: ParsedResourceMcpJson): void {
  const entryName =
    name.trim() || (isJsonRecord(value) ? (jsonStringField(value, "name") ?? "") : "");
  if (!looksLikeServerDef(value)) {
    out.skipped.push({ name: entryName, reason: "invalid" });
    return;
  }
  const hasCommand = typeof value.command === "string" && value.command.trim() !== "";
  const declared = value.type ?? value.transport;
  const normalized = normalizeJsonTransport(declared);
  if (typeof declared === "string" && declared.trim() !== "" && normalized === null) {
    out.skipped.push({ name: entryName, reason: "unsupported" });
    return;
  }
  const description = jsonStringField(value, "description");
  // 未声明类型时按形状推断:有 command 走 stdio,否则 url 走 http
  const transport = normalized ?? (hasCommand ? "stdio" : "http");
  if (transport === "stdio") {
    const command = hasCommand ? (value.command as string).trim() : "";
    if (!command) {
      out.skipped.push({ name: entryName, reason: "invalid" });
      return;
    }
    const args = jsonArgs(value.args);
    const env = jsonMap(value.env);
    out.entries.push({
      name: entryName,
      ...(description ? { description } : {}),
      transport,
      command,
      ...(args ? { args } : {}),
      ...(env ? { env } : {}),
    });
  } else {
    const url = (value.url as string).trim();
    if (!url) {
      out.skipped.push({ name: entryName, reason: "invalid" });
      return;
    }
    const headers = jsonMap(value.headers);
    out.entries.push({
      name: entryName,
      ...(description ? { description } : {}),
      transport,
      url,
      ...(headers ? { headers } : {}),
    });
  }
}

function collectJsonServerMap(map: JsonRecord, out: ParsedResourceMcpJson): void {
  for (const [name, value] of Object.entries(map)) {
    collectJsonServerDef(name, value, out);
  }
}

/**
 * 解析粘贴的 MCP JSON 配置为待导入条目。支持四种结构:
 * `{mcpServers: {...}}`(claude/cursor/.mcp.json)、`{servers: {...}}`(VS Code)、
 * 单个服务器定义对象(名称取自身 name 字段)、以及裸键值表 `{名称: 定义}`。
 */
export function parseResourceMcpJson(text: string): ParsedResourceMcpJson {
  let root: unknown;
  try {
    root = JSON.parse(text);
  } catch {
    throw new ResourceMcpJsonError("invalidJson");
  }
  const out: ParsedResourceMcpJson = { entries: [], skipped: [] };
  if (isJsonRecord(root)) {
    if (isJsonRecord(root.mcpServers)) {
      collectJsonServerMap(root.mcpServers, out);
      return out;
    }
    if (isJsonRecord(root.servers)) {
      collectJsonServerMap(root.servers, out);
      return out;
    }
    if (looksLikeServerDef(root)) {
      collectJsonServerDef(jsonStringField(root, "name") ?? "", root, out);
      return out;
    }
    // 裸键值表:全部值为对象且至少一个形似服务器定义时按名称导入,
    // 个别无效条目由 collectJsonServerDef 记入 skipped
    const values = Object.values(root);
    if (values.length > 0 && values.every(isJsonRecord) && values.some(looksLikeServerDef)) {
      collectJsonServerMap(root, out);
      return out;
    }
  }
  throw new ResourceMcpJsonError("unrecognized");
}

export function parseEnvLines(text: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const index = line.indexOf("=");
    if (index <= 0) continue;
    const key = line.slice(0, index).trim();
    if (key) env[key] = line.slice(index + 1).trim();
  }
  return env;
}

export function formatEnvLines(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

export function parseHeaderLines(text: string): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const index = line.indexOf(":");
    if (index <= 0) continue;
    const key = line.slice(0, index).trim();
    if (key) headers[key] = line.slice(index + 1).trim();
  }
  return headers;
}

export function formatHeaderLines(headers: Record<string, string>): string {
  return Object.entries(headers)
    .map(([key, value]) => `${key}: ${value}`)
    .join("\n");
}

export function parseArgLines(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith("#"));
}

export function formatArgLines(args: string[]): string {
  return args.join("\n");
}

export type ResourceBackupState = "never" | "idle" | "syncing" | "diverged" | "error";

export interface ResourceBackupStatus {
  configured: boolean;
  remoteUrl: string;
  branch: string;
  encrypted: boolean;
  unlocked: boolean;
  state: ResourceBackupState;
  lastSyncAt?: number;
  ahead?: number;
  behind?: number;
  error?: string;
}

type BackendSyncRecord = {
  at: number;
  ok: boolean;
  errorMessage?: string;
  ahead: number;
  behind: number;
  diverged: boolean;
};

type BackendLibraryInfo = {
  encrypted: boolean;
  unlocked: boolean;
  remoteUrl?: string;
  branch?: string;
  lastSync?: BackendSyncRecord;
};

type BackendSyncStatus = {
  remote?: string;
  branch?: string;
  ahead: number;
  behind: number;
  diverged: boolean;
  lastSync?: BackendSyncRecord;
};

type BackendSyncOutcome = {
  ok: boolean;
  ahead: number;
  behind: number;
  diverged: boolean;
  errorMessage?: string;
};

function mapBackupStatus(
  info: BackendLibraryInfo,
  sync?: BackendSyncStatus,
  outcome?: BackendSyncOutcome,
): ResourceBackupStatus {
  const record = sync?.lastSync ?? info.lastSync;
  const diverged = outcome?.diverged ?? sync?.diverged ?? record?.diverged ?? false;
  const failed = outcome ? !outcome.ok : record ? !record.ok : false;
  const remoteUrl = sync?.remote ?? info.remoteUrl ?? "";
  return {
    configured: remoteUrl.length > 0,
    remoteUrl,
    branch: sync?.branch ?? info.branch ?? "main",
    encrypted: info.encrypted,
    unlocked: !info.encrypted || info.unlocked,
    state: diverged ? "diverged" : failed ? "error" : record ? "idle" : "never",
    lastSyncAt: record?.at,
    ahead: outcome?.ahead ?? sync?.ahead ?? record?.ahead,
    behind: outcome?.behind ?? sync?.behind ?? record?.behind,
    error: outcome?.errorMessage ?? record?.errorMessage,
  };
}

async function getLibraryInfo(): Promise<BackendLibraryInfo> {
  return cmd<BackendLibraryInfo>("rl_library_info");
}

export function openResourceLibraryDir(): Promise<void> {
  return cmd<void>("rl_library_open_dir");
}

export async function getResourceBackupStatus(): Promise<ResourceBackupStatus> {
  const info = await getLibraryInfo();
  if (!info.remoteUrl) return mapBackupStatus(info);
  try {
    return mapBackupStatus(info, await cmd<BackendSyncStatus>("rl_sync_status"));
  } catch (error) {
    return { ...mapBackupStatus(info), state: "error", error: String(error) };
  }
}

export async function configureResourceBackup(
  remoteUrl: string,
  branch?: string,
): Promise<ResourceBackupStatus> {
  const outcome = await cmd<BackendSyncOutcome>("rl_remote_configure", {
    url: remoteUrl,
    branch,
  });
  return mapBackupStatus(await getLibraryInfo(), undefined, outcome);
}

export function unlinkResourceBackup(): Promise<void> {
  return cmd<void>("rl_remote_remove");
}

export async function syncResourceBackupNow(): Promise<ResourceBackupStatus> {
  const outcome = await cmd<BackendSyncOutcome>("rl_sync_once");
  return mapBackupStatus(await getLibraryInfo(), undefined, outcome);
}

export async function resolveResourceBackup(chooseRemote: boolean): Promise<ResourceBackupStatus> {
  await cmd<void>("rl_resolve_fork", { direction: chooseRemote ? "remote" : "local" });
  return syncResourceBackupNow();
}

export async function setResourceBackupEncryption(
  enabled: boolean,
  passphrase?: string,
): Promise<ResourceBackupStatus> {
  const outcome = await cmd<BackendSyncOutcome>(
    enabled ? "rl_encryption_enable" : "rl_encryption_disable",
    { password: passphrase ?? "" },
  );
  return mapBackupStatus(await getLibraryInfo(), undefined, outcome);
}

export async function unlockResourceBackup(passphrase: string): Promise<ResourceBackupStatus> {
  await cmd<void>("rl_encryption_unlock", { password: passphrase });
  return getResourceBackupStatus();
}

export async function lockResourceBackup(): Promise<ResourceBackupStatus> {
  await cmd<void>("rl_encryption_lock");
  return getResourceBackupStatus();
}

export function onResourceBackupStatusChanged(
  handler: (status: ResourceBackupStatus) => void,
): Promise<() => void> {
  return onListen<BackendSyncOutcome>("resource-library://sync-completed", () => {
    void getResourceBackupStatus().then(handler);
  });
}
