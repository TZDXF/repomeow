export type ToolchainKind = "rust" | "python" | "node" | "dotnet" | "git";

/** 版本管理器登记的一个版本(rustup 工具链 / nvm·fnm·vp 的 Node 版本 / dotnet SDK) */
export interface ToolchainVersion {
  name: string;
  /** 是否为当前生效的全局默认 */
  current: boolean;
}

/** 「添加版本」的远端可装版本(list_toolchain_versions) */
export interface ToolchainRemoteVersion {
  name: string;
  /** 版本线标记文字,直接取自数据源(nvm 表格列头 / vp 的 LTS·Current);无从判定为 null */
  tag: string | null;
}

/** 该工具在当前平台/安装来源下支持的操作(设置页按钮可见性) */
export interface ToolchainCaps {
  can_install: boolean;
  can_update: boolean;
  can_uninstall: boolean;
  /** 是否有版本管理能力(切换全局版本/装卸指定版本) */
  can_switch: boolean;
  /** 「添加版本」能否拉取远端可安装列表;否时前端退化为自由输入 */
  can_list_remote: boolean;
}

/** 单个工具链工具的检测结果(detect_toolchains) */
export interface ToolchainStatus {
  /** CLI 名(rustup / rustc / cargo / uv / nvm / fnm / vp / dotnet / git / gh) */
  id: string;
  kind: ToolchainKind;
  found: boolean;
  version: string | null;
  path: string | null;
  /** 安装来源:"winget" / "rustup" / "brew" / "standalone" */
  source: string | null;
  versions: ToolchainVersion[];
  /** gh:当前登录用户名(gh auth status 解析;未登录为 null) */
  account: string | null;
  caps: ToolchainCaps;
}

/** 工具链管理操作(toolchain_op 的 op 参数) */
export type ToolchainOp =
  | "install"
  | "update"
  | "uninstall"
  | "use"
  | "install_version"
  | "uninstall_version"
  | "login";
