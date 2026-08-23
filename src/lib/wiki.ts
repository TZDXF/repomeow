import { cmd } from "@/lib/tauri";
import type { WikiGenerationConfig } from "@/lib/wiki-generator";
import type { WikiData } from "@/types";

/** 项目的 wiki 目录路径(~/.repomeow/wiki/<basename>-<hash>/),仅展示用 */
export function getWikiDir(projectPath: string): Promise<string> {
  return cmd<string>("get_wiki_dir", { projectPath });
}

/** 读取项目 Wiki 目录中的独立生成配置；未配置时后端返回内置 API 默认值。 */
export function loadWikiConfig(projectPath: string): Promise<WikiGenerationConfig> {
  return cmd<WikiGenerationConfig>("load_wiki_config", { projectPath });
}

/** 将项目独立的生成配置保存为 Wiki 目录下的 config.json。 */
export function saveWikiConfig(projectPath: string, config: WikiGenerationConfig): Promise<void> {
  return cmd<void>("save_wiki_config", { projectPath, config });
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
