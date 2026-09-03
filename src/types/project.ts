import type { GitStatus } from "./git";
import type { JavaBuildGroup } from "./java";

export interface Tag {
  id: number;
  name: string;
  color: string;
}

export interface Project {
  id: number;
  path: string;
  name: string;
  description: string;
  tags: Tag[];
  git: GitStatus | null;
  /** 登记的目录当前是否仍存在(被移动/删除/盘符离线时为 false) */
  path_exists: boolean;
  archived_at: number | null;
  /** 收藏时间(null = 未收藏;列表中收藏项目置顶,组内按收藏时间倒序) */
  favorited_at: number | null;
  /** 跟踪更新:开启后远端有更新时后台自动快进拉取(无法快进即取消,不提醒) */
  auto_pull: boolean;
  /** Wiki 自动增量更新(项目级):本地 HEAD 变化且 relevantFiles 命中时自动增量更新 */
  wiki_auto_update: boolean;
  created_at: number;
  updated_at: number;
}

export interface PackageScript {
  name: string;
  command: string;
}

/** 一个 package.json 的 scripts 分组(monorepo 下可能有多个) */
export interface PackageScriptsGroup {
  /** package.json 所在目录的相对路径('/' 分隔),根目录为 "." */
  dir: string;
  /** package.json 的 name 字段,可能为空 */
  package_name: string | null;
  scripts: PackageScript[];
}

export interface CustomCommand {
  id: number;
  project_id: number;
  name: string;
  command: string;
  description: string;
  icon: string;
  sort_order: number;
}

/** 一条可浏览器访问的端口映射:宿主机发布端口 -> 容器端口 */
export interface ComposePort {
  /** 宿主机发布端口(浏览器访问入口) */
  published: number;
  /** 容器内目标端口 */
  target: number;
}

/** compose 文件中的一个服务及其对外可访问的宿主机端口 */
export interface ComposeService {
  name: string;
  /** 端口映射(按发布端口去重升序);仅含可浏览器访问的固定发布端口 */
  ports: ComposePort[];
}

export interface ComposeFile {
  /** 相对项目根的路径('/' 分隔),如 "compose.yml" 或 "deploy/app.yml" */
  path: string;
  file_name: string;
  services: ComposeService[];
}

/** `docker compose ps` 查询到的单个服务运行状态 */
export interface ComposeServiceState {
  name: string;
  running: boolean;
  /** 原始状态文案,如 "Up 2 hours" / "Exited (0) 5 minutes ago" */
  status: string;
}

/** scan_project_assets 一次返回的项目资产扫描结果(后端单次目录遍历同时产出) */
export interface ProjectAssets {
  package_scripts: PackageScriptsGroup[];
  compose_files: ComposeFile[];
  java_builds: JavaBuildGroup[];
}
