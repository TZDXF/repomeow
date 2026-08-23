import { cmd } from "@/lib/tauri";
import type { WikiData } from "@/types";

/** 项目的 wiki 目录路径(~/.repomeow/wiki/<basename>-<hash>/),仅展示用 */
export function getWikiDir(projectPath: string): Promise<string> {
  return cmd<string>("get_wiki_dir", { projectPath });
}

/** 读取整个 wiki;meta 缺失/损坏/未完结返回 null;附带 HEAD 比对的 stale 标记 */
export function loadWiki(projectPath: string): Promise<WikiData | null> {
  return cmd<WikiData | null>("load_wiki", { projectPath });
}

/** 项目是否已有 wiki 数据目录(删除项目时联动询问是否一并清理) */
export function hasWiki(projectPath: string): Promise<boolean> {
  return cmd<boolean>("has_wiki", { projectPath });
}

/** 删除项目的整个 wiki 目录 */
export function deleteWiki(projectPath: string): Promise<void> {
  return cmd<void>("delete_wiki", { projectPath });
}

/** 在系统文件管理器中打开项目的 wiki 目录 */
export function openWikiDir(projectPath: string): Promise<void> {
  return cmd<void>("open_wiki_dir", { projectPath });
}
