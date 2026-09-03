export interface FilePreview {
  /** 文本内容;二进制文件为 null */
  text: string | null;
  /** 文本是否因超过大小上限被截断 */
  truncated: boolean;
  /** 完整文本按固定 o200k_base 编码器统计的 token 数;二进制文件为 null */
  tokenCount: number | null;
}

/** 项目文件清单条目(list_project_files / search_project_files) */
export interface ProjectFileEntry {
  /** 项目相对路径('/' 分隔) */
  path: string;
  /** 是否被 .gitignore / .ignore 排除(灰显用) */
  ignored: boolean;
  /** 是否目录(list_project_files 逐层返回会包含目录,空目录可见) */
  isDir: boolean;
}

/** 全文搜索结果(search_project_text) */
export interface TextSearchLine {
  /** 1-based 行号 */
  line: number;
  /** 行内容(超长行为匹配附近窗口片段) */
  text: string;
}

export interface TextSearchHit {
  /** 项目相对路径('/' 分隔) */
  path: string;
  /** 该文件内的匹配总数 */
  count: number;
  /** 命中行(按行号升序) */
  lines: TextSearchLine[];
}

export interface TextSearchOutcome {
  /** 命中文件(按路径排序) */
  hits: TextSearchHit[];
  /** 是否因命中数/文件数上限被截断 */
  truncated: boolean;
}

/** 工作区待提交的一个变更文件(git_worktree_files,提交对话框变更预览用) */
export interface GitWorktreeFile {
  /** 仓库相对路径(重命名时为新路径) */
  path: string;
  /** 重命名前的旧路径(仅 status = R 时有值) */
  old_path: string | null;
  /** 变更类型:A 新增 / M 修改 / D 删除 / R 重命名 / T 类型变更 */
  status: string;
  /** 新增行数;二进制文件为 null */
  additions: number | null;
  /** 删除行数;二进制文件为 null */
  deletions: number | null;
  /** 是否未跟踪文件(勾选"包含未跟踪文件"才会被提交) */
  untracked: boolean;
}

/** 用户自定义 AI 提示词(~/.repomeow/prompts/*.md);空字符串表示使用内置默认模板 */
