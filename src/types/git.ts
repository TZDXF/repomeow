export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  ahead: number;
  behind: number;
  staged: number;
  /** 未暂存修改数(含冲突文件) */
  modified: number;
  untracked: number;
  /** 合并冲突文件数 */
  conflicted: number;
  remote_ahead: number;
  last_fetch_at: number | null;
  /** HEAD 最新提交时间(Unix 秒);无提交的仓库为 null */
  last_commit_at: number | null;
}

/** `git pull` 的结果:最新状态 + 产生的合并冲突文件(为空表示无冲突) */
export interface GitPullResult {
  status: GitStatus;
  conflicts: string[];
}

/** 本地分支与其 upstream 的跟踪差值(ahead=领先远端,behind=落后远端) */
export interface GitBranchTrack {
  /** 本地分支名(与 GitBranches.local 中一致) */
  name: string;
  /** upstream 短名(如 origin/dev);upstream 已删除时为 null */
  upstream: string | null;
  ahead: number;
  behind: number;
}

/** 本地/远程分支列表(remote 不含 origin/HEAD 这类符号引用;tracking 只收录配置了 upstream 的本地分支) */
export interface GitBranches {
  local: string[];
  remote: string[];
  tracking: GitBranchTrack[];
}

/** 一条 Git stash 记录，index 对应 `stash@{index}`。 */
export interface GitStash {
  index: number;
  oid: string;
  message: string;
  author: string;
  created_at: number;
}

/** 一个 git worktree(来自 `git worktree list --porcelain`) */
export interface GitWorktree {
  /** 绝对路径(git 输出,Windows 上为 '/' 分隔) */
  path: string;
  /** 检出的短分支名;detached 时为 null */
  branch: string | null;
  /** HEAD 完整 hash */
  head: string;
  /** 是否主工作区 */
  is_main: boolean;
  /** 是否 detached HEAD */
  detached: boolean;
  /** 创建来源分支(新建分支时记录;无记录回退上游跟踪分支,如 origin/x;都没有为 null) */
  base_branch: string | null;
  /** 来源分支领先 HEAD 的提交数(>0 表示变基可带入新提交;无来源或引用已删除为 null) */
  base_behind: number | null;
}

/** `git merge` 的结果:最新状态 + 产生的合并冲突文件(为空表示无冲突) */
export interface GitMergeResult {
  status: GitStatus;
  conflicts: string[];
  /** 实际执行合并的工作区路径;目标分支未被检出时走快进,为空串 */
  merged_in: string;
}

/** `git rebase` 的结果:最新状态 + 冲突文件 + 变基是否处于中断状态 */
export interface GitRebaseResult {
  status: GitStatus;
  conflicts: string[];
  in_progress: boolean;
}

/** 批量 git 状态查询结果中的一项(按 path 与项目列表对应) */
export interface GitStatusItem {
  path: string;
  status: GitStatus;
}

export type GitCheckScope =
  | { kind: "all" }
  | { kind: "project"; projectId: number }
  | { kind: "path"; path: string };

/** 统一 Git 状态事件:后台检查与应用内写操作均通过此协议发布。 */
export interface GitProjectChangedPayload {
  project_id: number | null;
  name: string | null;
  path: string;
  status: GitStatus;
  head_sha: string | null;
  head_changed: boolean;
  auto_pulled: boolean;
  pulled_commits: number;
  source: string;
  wiki_auto_update: boolean;
}

/** Rust 后台任务统一进度事件（定时报告等）。 */
export interface BackgroundTaskProgressPayload {
  task_id: string;
  kind: "report" | "wiki" | string;
  label: string;
  completed: number;
  total: number;
  status: "running" | "finished";
  project_id?: number | null;
}

/** 本地 agent 冲突解决后台任务的完成事件。 */
export interface ConflictResolutionFinishedPayload {
  task_id: string;
  project_id: number;
  path: string;
  success: boolean;
  remaining: string[];
  error: string | null;
}

/** 一个 git remote 及其地址 */
export interface GitRemote {
  name: string;
  url: string;
}

/** 一个可读取文本内容的未跟踪新文件(二进制/超限文件不在此列) */
export interface GitUntrackedFile {
  path: string;
  content: string;
  /** 内容是否因超长被截断 */
  truncated: boolean;
}

/** 生成提交信息所需的变更上下文(diff 可能已被截断) */
export interface GitCommitContext {
  /** `git diff --stat` 摘要 */
  stat: string;
  /** 相对 HEAD 的完整 diff(超长时截断;已排除锁文件等噪声) */
  diff: string;
  /** diff 是否因超长被截断 */
  truncated: boolean;
  /** 全部未跟踪文件名(含无内容的,供模型感知新增文件) */
  untracked: string[];
  /** 未跟踪文件中可读取的文本内容(跳过二进制与超限文件) */
  untracked_files: GitUntrackedFile[];
}

/** 一条 git 提交记录(日报生成用) */
export interface GitCommitInfo {
  hash: string;
  author: string;
  /** 本地时间 "YYYY-MM-DD HH:MM" */
  date: string;
  subject: string;
}

/** 提交图谱中的一条提交(含分支拓扑所需的父提交与引用装饰) */
export interface GitGraphCommit {
  /** 完整 hash */
  hash: string;
  /** 父提交 hash(合并提交有多个,根提交为空) */
  parents: string[];
  author: string;
  /** 本地时间 "YYYY-MM-DD HH:MM" */
  date: string;
  subject: string;
  /** 指向该提交的引用:"main"、"origin/main"、"tag: v1.0"(tag 保留前缀) */
  refs: string[];
  /** HEAD 是否指向此提交 */
  is_head: boolean;
}

/** 仓库当前 git 用户身份(user.name / user.email) */
export interface GitUser {
  name: string;
  email: string;
}

/** 某次提交触及的一个文件(git_commit_files) */
export interface GitCommitFile {
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
}

/** 某次提交中单个文件的 diff(git_commit_file_diff) */
export interface GitCommitFileDiff {
  diff: string;
  /** diff 是否因超长被截断 */
  truncated: boolean;
}

/** 一位提交者的统计(git_project_stats;email 归并,展示名为最近一次使用的名字) */
export interface GitAuthorStat {
  name: string;
  email: string;
  commits: number;
  additions: number;
  deletions: number;
  firstCommitAt: number;
  lastCommitAt: number;
}

/** 一天的提交聚合;t 的 UTC 日期即提交者本地日期(仅作日历标识,非真实时刻) */
export interface GitDayStat {
  t: number;
  count: number;
  additions: number;
  deletions: number;
}

/** 单次非合并提交的增删行(代码变更趋势逐提交 K 线的数据点) */
export interface GitCommitChurn {
  /** committer 时间(Unix 秒,真实时刻;与 GitDayStat 仅作日历标识的 t 不同) */
  t: number;
  /** 短 hash(前 7 位) */
  shortId: string;
  /** 提交信息首行(截断 80 字符) */
  subject: string;
  additions: number;
  deletions: number;
}

/** HEAD 树上一种扩展名的文件分布 */
export interface GitFileTypeStat {
  /** 小写扩展名(不含点);无扩展名的清单文件按文件名(如 dockerfile),其余进 "(other)" */
  ext: string;
  files: number;
  bytes: number;
}

/** 项目级 git 统计(git_project_stats,图谱页数据分析面板用);全量历史聚合 */
export interface GitProjectStats {
  totalCommits: number;
  mergeCommits: number;
  /** 最早/最新提交的 committer 时间(Unix 秒);空仓库为 null */
  firstCommitAt: number | null;
  lastCommitAt: number | null;
  /** 有提交的天然日数量(按提交者本地时区) */
  activeDays: number;
  /** 累计增删行(仅非合并提交,且受 churn 上限约束;二进制不计) */
  totalAdditions: number;
  totalDeletions: number;
  /** true 时增删行只覆盖最近一部分提交(提交数超过 churn 统计上限) */
  churnTruncated: boolean;
  /** 逐提交增删行(仅非合并提交,受 churn 上限约束),按 committer 时间升序 */
  churnCommits: GitCommitChurn[];
  /** 按提交数降序 */
  authors: GitAuthorStat[];
  /** 7*24 行主序:行 = 周一..周日,列 = 0..23 时(提交者本地时间) */
  weekdayHour: number[];
  /** 按天然日升序 */
  byDay: GitDayStat[];
  /** HEAD 树按扩展名分布,按字节数降序 */
  fileTypes: GitFileTypeStat[];
  totalFiles: number;
  totalBytes: number;
}
