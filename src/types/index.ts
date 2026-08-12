export interface Tag {
  id: number;
  name: string;
  color: string;
}

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
}

/** `git merge` 的结果:最新状态 + 产生的合并冲突文件(为空表示无冲突) */
export interface GitMergeResult {
  status: GitStatus;
  conflicts: string[];
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
  /** 最近提交信息 subject(风格锚定用,新仓库为空) */
  recent_commits: string[];
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

/** 用户自定义 AI 提示词(~/.repomeow/prompts/*.md);空字符串表示使用内置默认模板 */
export interface AiPrompts {
  /** 提交信息生成提示词 */
  commit: string;
  /** 日报生成提示词 */
  report: string;
  /** 周报生成提示词 */
  reportWeekly: string;
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

export interface ReadmeContent {
  file_name: string;
  content: string;
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
}

export type EditorKind =
  | "explorer"
  | "vscode"
  | "cursor"
  | "windsurf"
  | "trae"
  | "vscodium"
  | "zed"
  | "sublime"
  | "idea"
  | "webstorm"
  | "goland"
  | "pycharm"
  | "clion"
  | "rustrover"
  | "terminal";

/** 可隐藏的 UI 项类型:package.json 分组 / 分组内单条命令 / compose 文件 */
export type HiddenKind = "packageFile" | "packageScript" | "composeFile";

/** 项目维度被隐藏的 UI 项(targetKey 含义见各使用处) */
export interface HiddenItem {
  kind: HiddenKind;
  targetKey: string;
}

/** 可标记为「常用」的命令类型 */
export type PinKind = "packageScript" | "composeFile" | "composeService" | "customCommand";

/**
 * 一条被标记为「常用」的命令,在托盘弹窗项目列表中可直接执行
 * target_key: packageScript = "<dir>\n<name>";composeFile = 文件相对路径;
 *             composeService = "<file>\n<service>";customCommand = 命令 id
 * command: npm/自定义为完整命令;compose 类为基础前缀 `docker compose -f "..."`,动作在执行时拼接
 */
export interface PinnedCommand {
  id: number;
  project_id: number;
  kind: PinKind;
  target_key: string;
  label: string;
  command: string;
  /** 可选工作目录:相对项目根(monorepo 子包),执行时拼接 project.path,迁移目录后仍可用 */
  cwd: string | null;
  /** 自定义命令的图标名(后端 list 时实时 JOIN custom_commands,其他 kind 为 null) */
  icon: string | null;
  created_at: number;
}

export interface GitUpdatedPayload {
  project_id: number;
  remote_ahead: number;
  last_fetch_at: number;
}

/** 报告类型:日报(单日) | 周报(日期范围) */
export type ReportPeriodType = "daily" | "weekly";

/** 工作周日期范围(get_work_week_ranges,起止均为 "YYYY-MM-DD") */
export interface WorkWeekRange {
  from: string;
  to: string;
}

/** 本周/上周工作周范围(连续工作周期,含法定节假日/调休识别) */
export interface WorkWeekRanges {
  thisWeek: WorkWeekRange;
  lastWeek: WorkWeekRange;
}

/** 报告历史列表项 */
export interface ReportHistoryItem {
  id: number;
  projectIds: number[];
  dateFrom: string;
  dateTo: string;
  rangeLabel: string;
  authorMode: string;
  language: string;
  periodType: ReportPeriodType;
  createdAt: number;
  projectNames: string[];
  totalCommits: number;
}

/** 报告历史详情(含 Markdown 正文与各项目提交记录) */
export interface ReportHistoryDetail {
  id: number;
  projectIds: number[];
  dateFrom: string;
  dateTo: string;
  rangeLabel: string;
  authorMode: string;
  language: string;
  periodType: ReportPeriodType;
  createdAt: number;
  projectNames: string[];
  totalCommits: number;
  result: string;
  commits: ReportCommitItem[];
}

/** 报告历史中单个项目的提交记录 */
export interface ReportCommitItem {
  projectId: number | null;
  projectName: string;
  projectDescription: string;
  commits: GitCommitInfo[];
}

/** 保存报告时传入的提交数据 */
export interface SaveReportCommit {
  projectId: number | null;
  projectName: string;
  projectDescription?: string;
  commits: GitCommitInfo[];
}

/** 定时任务配置 */
export interface ReportSchedule {
  id: string;
  name: string;
  enabled: boolean;
  /** 报告类型:日报(当天) | 周报(工作周,最后一个工作日触发) */
  reportType: ReportPeriodType;
  projectIds: number[];
  authorMode: "me" | "all";
  timeOfDay: string;
  /** 日报:仅周一~周五 */
  weekdaysOnly: boolean;
  /** 日报:仅中国工作日 */
  chineseWorkdayOnly: boolean;
  /** 周报:true = 工作周模式(自动识别连续工作周期,末日触发);false = 自定义周几~周几 */
  weeklyWorkweek: boolean;
  /** 周报自定义:范围起始周几(1=周一 .. 7=周日) */
  weeklyStartWeekday: number;
  /** 周报自定义:范围结束/触发周几(1=周一 .. 7=周日) */
  weeklyEndWeekday: number;
  lastRunAt: number | null;
}

/** 定时任务触发后发送给前端的通知 */
export interface ReportGeneratedPayload {
  scheduleName: string;
  historyId: number;
  dateFrom: string;
  dateTo: string;
}

/** 日历标注数据：某月每天报告数 + 节假日/调休 */
export interface CalendarMeta {
  dates: Record<string, number>;
  holidays: string[];
  workdays: string[];
}

/** 节假日/调休标注数据(get_holiday_data 返回的全集,供日期选择日历高亮) */
export interface HolidayData {
  holidays: string[];
  workdays: string[];
}

/** 批量生成的单个时段(plan_batch_report_ranges;daily 为单日,weekly 为一个工作周) */
export interface BatchRange {
  dateFrom: string;
  dateTo: string;
  isWorkday: boolean;
}

/** 已有报告的日期范围(list_report_dates,供批量生成"跳过已有"匹配) */
export interface ReportDateRange {
  dateFrom: string;
  dateTo: string;
}
