import { cmd } from "@/lib/tauri";
import type {
  WikiChangedFiles,
  WikiCommitKind,
  WikiContext,
  WikiData,
  WikiFileContent,
  WikiMeta,
} from "@/types";

/** 收集 wiki 结构阶段输入:过滤后的文件树 + README + 清单文件 + 当前 HEAD */
export function collectWikiContext(projectPath: string): Promise<WikiContext> {
  return cmd<WikiContext>("collect_wiki_context", { projectPath });
}

/** 批量读取单页生成所需的相关文件全文(读不到/二进制文件被静默跳过) */
export function readWikiFiles(projectPath: string, relPaths: string[]): Promise<WikiFileContent[]> {
  return cmd<WikiFileContent[]>("read_wiki_files", { projectPath, relPaths });
}

/** 项目的 wiki 目录路径(~/.repomeow/wiki/<basename>-<hash>/),仅展示用 */
export function getWikiDir(projectPath: string): Promise<string> {
  return cmd<string>("get_wiki_dir", { projectPath });
}

/** 开始一次全新生成:清空旧 pages/ 与 meta.json(保留 .git 历史) */
export function beginWiki(projectPath: string): Promise<void> {
  return cmd<void>("begin_wiki", { projectPath });
}

/** 写入单个页面(tmp + rename);fileName 必须匹配 `NN-slug.md` */
export function saveWikiPage(
  projectPath: string,
  fileName: string,
  content: string,
): Promise<void> {
  return cmd<void>("save_wiki_page", { projectPath, fileName, content });
}

/**
 * 写入 meta.json(最后调用;version 与 generatedAt 由后端覆写),落盘后自动做一次
 * git 快照提交(commitKind 决定提交信息;提交失败不影响落盘结果)
 */
export function saveWikiMeta(
  projectPath: string,
  meta: WikiMeta,
  commitKind: WikiCommitKind = "generate",
): Promise<void> {
  return cmd<void>("save_wiki_meta", { projectPath, meta, commitKind });
}

/**
 * 手动触发一次 wiki 目录的 git 快照提交(单页重新生成等不经 saveWikiMeta 的场景);
 * kind="page" 时传页面标题用于组提交信息
 */
export function commitWiki(
  projectPath: string,
  kind: WikiCommitKind,
  title?: string,
): Promise<void> {
  return cmd<void>("commit_wiki", { projectPath, kind, title: title ?? null });
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

/** 增量更新用:列出 fromSha..HEAD 之间变更的文件与当前 HEAD;非 git 仓库返回空表 */
export function wikiChangedFiles(projectPath: string, fromSha: string): Promise<WikiChangedFiles> {
  return cmd<WikiChangedFiles>("wiki_changed_files", { projectPath, fromSha });
}
