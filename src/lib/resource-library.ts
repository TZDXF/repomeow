import { cmd, onListen } from "@/lib/tauri";

/**
 * 设置页全局资源库的唯一 IPC 桥接层。
 * 数据由后端固定存放在 ~/.repomeow/resource-library，前端不参与路径计算。
 */

export interface ResourceSkillGroup {
  id: string;
  name: string;
  color?: string;
  sortOrder: number;
}

export interface ResourceSkill {
  id: string;
  name: string;
  description: string;
  directory: string;
  /** 来自 skills.sh 的来源元数据；手动创建 Skill 不带此字段。 */
  marketplace?: { id: string; source: string; url: string };
  groupIds: string[];
  sortOrder: number;
  updatedAt?: number;
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

export function createResourceSkill(input: ResourceSkillInput): Promise<ResourceSkill> {
  return cmd<ResourceSkill>("rl_skill_create", {
    name: input.name,
    description: input.description,
    groupIds: input.groupIds,
    body: input.body,
  });
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

export async function readResourceSkillBody(id: string): Promise<{ body: string }> {
  const result = await cmd<{ content: string }>("rl_skill_body_read", { id });
  return { body: result.content };
}

export function saveResourceSkillBody(id: string, body: string): Promise<void> {
  return cmd<void>("rl_skill_body_write", { id, content: body });
}

export function deleteResourceSkill(id: string): Promise<void> {
  return cmd<void>("rl_skill_delete", { id });
}

export function reorderResourceSkills(orderedIds: string[]): Promise<void> {
  return cmd<void>("rl_skill_reorder", { ids: orderedIds });
}

export function openResourceSkillDir(id: string): Promise<void> {
  return cmd<void>("rl_skill_open_dir", { id });
}

export async function createResourceSkillGroup(
  name: string,
  color?: string,
): Promise<ResourceSkillGroup> {
  return mapGroup(await cmd<BackendSkillGroup>("rl_skill_group_create", { name, color }));
}

export async function updateResourceSkillGroup(
  id: string,
  name: string,
  color?: string,
): Promise<ResourceSkillGroup> {
  return mapGroup(await cmd<BackendSkillGroup>("rl_skill_group_rename", { id, name, color }));
}

export function deleteResourceSkillGroup(id: string): Promise<void> {
  return cmd<void>("rl_skill_group_delete", { id });
}

export function reorderResourceSkillGroups(orderedIds: string[]): Promise<void> {
  return cmd<void>("rl_skill_group_reorder", { ids: orderedIds });
}

export function filterSkills(
  skills: ResourceSkill[],
  query: string,
  groupId: string | null,
): ResourceSkill[] {
  const q = query.trim().toLowerCase();
  return skills.filter((skill) => {
    if (groupId !== null && !skill.groupIds.includes(groupId)) return false;
    if (!q) return true;
    return skill.name.toLowerCase().includes(q) || skill.description.toLowerCase().includes(q);
  });
}

export function mergeReorderedVisible(allIds: string[], visibleNewOrder: string[]): string[] {
  const visibleSet = new Set(visibleNewOrder);
  const others = allIds.filter((id) => !visibleSet.has(id));
  const anchorIndex = allIds.findIndex((id) => visibleSet.has(id));
  const insertAt = anchorIndex === -1 ? others.length : anchorIndex;
  return [...others.slice(0, insertAt), ...visibleNewOrder, ...others.slice(insertAt)];
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
