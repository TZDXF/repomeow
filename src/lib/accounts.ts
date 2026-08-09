import { cmd } from "@/lib/tauri";

/** 代码托管平台 */
export type Provider = "github" | "gitee" | "gitlab";

/** 已绑定的平台账号(token 不回传,只有脱敏预览) */
export interface GitAccount {
  id: number;
  provider: Provider;
  label: string;
  baseUrl: string;
  username: string;
  tokenPreview: string;
  /** 拉取仓库遇到 401 时由后端置 true,设置页据此显示「Token 已失效」标记 */
  tokenInvalid: boolean;
  createdAt: number;
  updatedAt: number;
}

/** 平台账号下的远程仓库(后端已把各平台字段归一化) */
export interface RemoteRepo {
  repoId: string;
  /** 所属组织/用户名(namespace) */
  owner: string;
  name: string;
  fullName: string;
  description: string;
  htmlUrl: string;
  httpCloneUrl: string;
  sshCloneUrl: string;
  defaultBranch: string;
  isPrivate: boolean;
  updatedAt: string;
}

export function listGitAccounts(): Promise<GitAccount[]> {
  return cmd<GitAccount[]>("list_git_accounts");
}

export function addGitAccount(input: {
  provider: Provider;
  label: string;
  baseUrl?: string;
  token: string;
}): Promise<GitAccount> {
  return cmd<GitAccount>("add_git_account", {
    provider: input.provider,
    label: input.label,
    baseUrl: input.baseUrl ?? null,
    token: input.token,
  });
}

/** token 传空字符串/省略表示保留原 token */
export function updateGitAccount(input: {
  id: number;
  label: string;
  baseUrl?: string;
  token?: string;
}): Promise<GitAccount> {
  return cmd<GitAccount>("update_git_account", {
    id: input.id,
    label: input.label,
    baseUrl: input.baseUrl ?? null,
    token: input.token?.trim() ? input.token : null,
  });
}

export function removeGitAccount(id: number): Promise<void> {
  return cmd("remove_git_account", { id });
}

/** 一次拉取账号下全部仓库(后端循环分页,上限 1000 条) */
export function listAccountRepos(accountId: number): Promise<RemoteRepo[]> {
  return cmd<RemoteRepo[]>("list_account_repos", { accountId });
}

/**
 * 探测 GitHub CLI(gh)虚拟账号:已安装且已登录时返回(id 固定为 0),
 * 否则返回 null;由「账号仓库」下拉并入,不落库、不出现在设置页账号列表
 */
export function getGhCliAccount(): Promise<GitAccount | null> {
  return cmd<GitAccount | null>("get_gh_cli_account");
}

/** 所有未归档项目的 origin 地址,用于「已添加」匹配 */
export function listProjectRemoteUrls(): Promise<string[]> {
  return cmd<string[]>("list_project_remote_urls");
}

/**
 * 把 git remote URL 归一化为 "host/owner/repo" 小写形式,用于跨协议匹配。
 * 支持 https://host/owner/repo(.git)、http、git@host:owner/repo.git、ssh://git@host/owner/repo
 */
export function normalizeRemoteUrl(raw: string): string {
  let s = raw.trim();
  if (!s) return "";
  // ssh://git@host[:port]/owner/repo
  s = s.replace(/^ssh:\/\/git@/, "https://");
  // git@host:owner/repo.git (scp-like)
  const scp = s.match(/^git@([^:/]+):(.+)$/);
  if (scp) s = `https://${scp[1]}/${scp[2]}`;
  // 去掉协议与可能的 userinfo
  s = s.replace(/^https?:\/\//, "");
  s = s.replace(/^[^@/]+@/, "");
  // 去掉端口、末尾 .git 与多余斜杠
  s = s.replace(/^([^/:]+):\d+\//, "$1/");
  s = s.replace(/[\\/]+$/, "").replace(/\.git$/i, "");
  return s.toLowerCase();
}
