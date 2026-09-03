export interface AiPrompts {
  /** 提交信息生成提示词 */
  commit: string;
  /** 日报生成提示词 */
  report: string;
  /** 周报生成提示词 */
  reportWeekly: string;
}

// ── 项目 Wiki(~/.repomeow/wiki/<basename>-<hash>/ 下的 meta.json + pages/*.md) ──

/** 触发 wiki git 快照提交的操作类型(后端据此组提交信息) */
/** wiki 大纲中的单个页面条目 */
export interface WikiOutlinePage {
  id: string;
  /** 页面文件名(pages/ 下,如 `01-overview.md`) */
  file: string;
  title: string;
  /** 该页覆盖内容的简述(大纲阶段产出,单页生成时注入 prompt) */
  description: string;
  section: string | null;
  importance: string;
  relevantFiles: string[];
  relatedPages: string[];
}

/** wiki 元信息(meta.json);generatedAt 与 version 由后端覆写 */
export interface WikiMeta {
  version: number;
  projectPath: string;
  generatedAt: string;
  headSha: string | null;
  model: string;
  language: string;
  status: string;
  outline: WikiOutlinePage[];
  /** 生成后端标识("builtin" / "acp:<agentId>";旧 meta 缺省视为内置)。
   *  手动增量更新遇后端切换时退化为整本重生成 */
  generator?: string | null;
}

/** 一个已生成的 wiki 页面(含正文) */
export interface WikiPageData extends WikiOutlinePage {
  /** 页面 Markdown 正文;文件缺失时为空串 */
  content: string;
}

export interface WikiData {
  meta: WikiMeta;
  pages: WikiPageData[];
  /** 生成时的 HEAD 与当前 HEAD 不一致(代码已更新,wiki 可能过时) */
  stale: boolean;
}
