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

/** 单个文件的预览内容(read_file_preview) */
export interface FilePreview {
  /** 文本内容;二进制文件为 null */
  text: string | null;
  /** 文本是否因超过大小上限被截断 */
  truncated: boolean;
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
export type WikiCommitKind = "generate" | "update" | "page";

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

/** 结构阶段输入:collect_wiki_context 的返回 */
export interface WikiContext {
  /** 过滤后的文件树(每行一个 / 分隔相对路径,超预算时目录折叠为摘要行) */
  fileTree: string;
  /** 过滤后的完整文件清单(/ 分隔,不折叠),用于校验大纲标注的相关文件 */
  paths: string[];
  fileCount: number;
  treeTruncated: boolean;
  readme: string | null;
  manifests: { path: string; content: string }[];
  headSha: string | null;
}

/** wiki_changed_files 的返回:区间变更文件 + 提交数 + 当前 HEAD */
export interface WikiChangedFiles {
  files: string[];
  /** fromSha..HEAD 的提交数(自动增量更新按「未同步提交数达阈值」触发) */
  commitCount: number;
  headSha: string | null;
}

/** 单页生成的相关文件内容(read_wiki_files 返回;读不到/二进制文件被静默跳过) */
export interface WikiFileContent {
  path: string;
  content: string;
  truncated: boolean;
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
  /** Wiki 自动增量更新(项目级):跟踪拉取后未同步提交数达全局阈值时自动增量更新 */
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

/** Java 构建工具类型 */
export type JavaBuildTool = "maven" | "gradle";

/** 「更多操作」下拉里的一条常用命令(maven/gradle 生命周期目标) */
export interface JavaCommandAction {
  /** 前端 i18n 键(java.clean / java.package / ...) */
  key: string;
  command: string;
}

/** 一个 Spring Boot 构建文件的运行分组(monorepo 下可能有多个) */
export interface JavaBuildGroup {
  /** 构建文件所在目录的相对路径('/' 分隔),根目录为 "." */
  dir: string;
  tool: JavaBuildTool;
  /** 运行命令应执行的工作目录的相对路径(多模块工程统一在项目根执行) */
  run_dir: string;
  /** 平台相关的运行命令(优先项目内 wrapper,否则用 PATH 上的 mvn/gradle) */
  run_command: string;
  /** 常用操作(clean/package/install/test 等),与 run_command 同一执行目录 */
  more_actions: JavaCommandAction[];
}

/** 用户在设置页登记的 JDK(开发环境配置) */
export interface JdkConfig {
  id: string;
  name: string;
  /** JDK 根目录(JAVA_HOME) */
  path: string;
}

/** 自动探测到的 JDK 候选(detect_jdks);install_jdk 安装成功也返回同一结构 */
export interface JdkCandidate {
  path: string;
  /** `java -version` 解析出的版本串,如 "17.0.2" / "1.8.0_392" */
  version: string;
}

/** 在线安装源(list_remote_jdks / install_jdk 的 vendor 参数) */
export type JdkVendor = "adoptium" | "zulu";

/** 某安装源可在线安装的 JDK 大版本(list_remote_jdks) */
export interface RemoteJdkRelease {
  /** 主版本号(8 / 11 / 17 / 21 / 25 ...) */
  major: number;
  /** 该主版本当前最新的完整版本串,如 "17.0.20+8" */
  version: string;
}

/** 工具链所属生态(detect_toolchains 输出的分组) */
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

/** 用户在设置中配置的外部打开方式。命令支持 {path} 与 {line} 占位符。 */
export interface CustomOpenWith {
  id: string;
  name: string;
  command: string;
  icon: string;
}

/** 内置打开方式或以 custom: 前缀标识的自定义打开方式。 */
export type OpenWithId = EditorKind | `custom:${string}`;

/** 可隐藏的 UI 项类型:package.json 分组 / 分组内单条命令 / compose 文件 / Spring Boot 构建分组 */
export type HiddenKind = "packageFile" | "packageScript" | "composeFile" | "javaBuild";

/** 项目维度被隐藏的 UI 项(targetKey 含义见各使用处) */
export interface HiddenItem {
  kind: HiddenKind;
  targetKey: string;
}

/** 详情页首屏聚合数据(get_project_overview 一次 IPC 返回) */
export interface ProjectOverview {
  hidden_items: HiddenItem[];
  custom_commands: CustomCommand[];
}

/** 可标记为「常用」的命令类型 */
export type PinKind =
  | "packageScript"
  | "composeFile"
  | "composeService"
  | "customCommand"
  | "javaBuild";

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

/** git://auto-pulled 事件载荷:跟踪更新快进拉取成功(pulled = 本轮拉取的提交数) */
export interface GitAutoPulledPayload {
  project_id: number;
  pulled: number;
}

/** 报告类型:日报(单日) | 周报(日期范围) */
export type ReportPeriodType = "daily" | "weekly";

/** 报告历史日历的选中视角:按日 | 按周(周一至周日) | 按月 */
export type ReportViewMode = "day" | "week" | "month";

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
  /** 报告类型:日报(前一天或当天) | 周报(工作周,最后一个工作日触发) */
  reportType: ReportPeriodType;
  projectIds: number[];
  /** 按标签动态包含:执行时反查带有任一选中标签的未归档项目,与 projectIds 取并集 */
  tagIds: number[];
  authorMode: "me" | "all";
  timeOfDay: string;
  /** 日报:仅周一~周五 */
  weekdaysOnly: boolean;
  /** 日报:仅中国工作日 */
  chineseWorkdayOnly: boolean;
  /** 日报:true = 前一天(次日生成,默认);false = 当天 */
  previousDay: boolean;
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
