use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitStatus {
    pub is_repo: bool,
    pub branch: Option<String>,
    pub ahead: i32,
    pub behind: i32,
    pub staged: i32,
    /// 未暂存修改数(含冲突文件,保证「工作区干净」判断不变)
    pub modified: i32,
    pub untracked: i32,
    /// 合并冲突文件数(porcelain 的 u 条目)
    pub conflicted: i32,
    pub remote_ahead: i32,
    pub last_fetch_at: Option<i64>,
    /// HEAD 最新提交时间(Unix 秒);无提交的仓库为 None
    pub last_commit_at: Option<i64>,
}

/// `git pull` 的结果:最新状态 + 产生的合并冲突文件(为空表示无冲突)
#[derive(Debug, Clone, Serialize)]
pub struct GitPullResult {
    pub status: GitStatus,
    pub conflicts: Vec<String>,
}

/// 本地分支与其 upstream 的跟踪差值(ahead=领先远端,behind=落后远端)
#[derive(Debug, Clone, Serialize)]
pub struct GitBranchTrack {
    /// 本地分支名(与 GitBranches::local 中一致)
    pub name: String,
    /// upstream 短名(如 origin/dev);upstream 已删除([gone])时为 None
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

/// 本地/远程分支列表(remote 不含 origin/HEAD 这类符号引用;
/// tracking 只收录配置了 upstream 的本地分支)
#[derive(Debug, Clone, Serialize)]
pub struct GitBranches {
    pub local: Vec<String>,
    pub remote: Vec<String>,
    pub tracking: Vec<GitBranchTrack>,
}

/// 一个 git worktree(来自 `git worktree list --porcelain`)
#[derive(Debug, Clone, Serialize)]
pub struct GitWorktree {
    /// 绝对路径(git 输出,Windows 上为 '/' 分隔)
    pub path: String,
    /// 检出的短分支名;detached 时为 None
    pub branch: Option<String>,
    /// HEAD 完整 hash
    pub head: String,
    /// 是否主工作区(porcelain 输出的第一条)
    pub is_main: bool,
    /// 是否 detached HEAD
    pub detached: bool,
    /// 创建来源分支:新建分支时记录在 `branch.<name>.repomeow-base`;
    /// 无记录时回退为上游跟踪分支(origin/x 形式);都没有则为 None
    pub base_branch: Option<String>,
    /// 来源分支领先 HEAD 的提交数(>0 表示变基可带入新提交);
    /// 无来源分支或来源引用无法解析(已删除等)时为 None
    pub base_behind: Option<usize>,
}

/// `git merge` 的结果:最新状态 + 产生的合并冲突文件(为空表示无冲突)
#[derive(Debug, Clone, Serialize)]
pub struct GitMergeResult {
    pub status: GitStatus,
    pub conflicts: Vec<String>,
    /// 实际执行合并的工作区路径;目标分支未被任何 worktree 检出时走快进,
    /// 不产生工作区改动,此字段为空串
    pub merged_in: String,
}

/// `git rebase` 的结果:最新状态 + 冲突文件 + 变基是否处于中断状态(待解决后继续/中止)
#[derive(Debug, Clone, Serialize)]
pub struct GitRebaseResult {
    pub status: GitStatus,
    pub conflicts: Vec<String>,
    pub in_progress: bool,
}

/// 一个可读取文本内容的未跟踪新文件(二进制/超限文件不会出现在此)
#[derive(Debug, Clone, Serialize)]
pub struct GitUntrackedFile {
    pub path: String,
    pub content: String,
    /// 内容是否因超长被截断
    pub truncated: bool,
}

/// 生成提交信息所需的变更上下文(diff 可能已被截断)
#[derive(Debug, Clone, Serialize)]
pub struct GitCommitContext {
    /// `git diff --stat` 摘要
    pub stat: String,
    /// 相对 HEAD 的完整 diff(超长时截断并追加标记;已排除锁文件等噪声)
    pub diff: String,
    /// diff 是否因超长被截断
    pub truncated: bool,
    /// 全部未跟踪文件名(含无内容的,供模型感知新增文件)
    pub untracked: Vec<String>,
    /// 未跟踪文件中可读取的文本内容(跳过二进制与超限文件)
    pub untracked_files: Vec<GitUntrackedFile>,
    /// 最近提交信息 subject(风格锚定用,新仓库为空)
    pub recent_commits: Vec<String>,
}

/// 一条 git 提交记录(日报生成用)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitInfo {
    pub hash: String,
    pub author: String,
    /// 本地时间 "YYYY-MM-DD HH:MM"
    pub date: String,
    pub subject: String,
}

/// 提交图谱中的一条提交(含分支拓扑所需的父提交与引用装饰)
#[derive(Debug, Clone, Serialize)]
pub struct GitGraphCommit {
    /// 完整 hash
    pub hash: String,
    /// 父提交 hash(合并提交有多个,根提交为空)
    pub parents: Vec<String>,
    pub author: String,
    /// 本地时间 "YYYY-MM-DD HH:MM"
    pub date: String,
    pub subject: String,
    /// 指向该提交的引用:"main"、"origin/main"、"tag: v1.0"(tag 保留前缀供前端区分)
    pub refs: Vec<String>,
    /// HEAD 是否指向此提交
    pub is_head: bool,
}

/// git_graph_log 流式输出的一个批次;done 为 true 表示提交序列结束(commits 可能为空)
#[derive(Debug, Clone, Serialize)]
pub struct GitGraphBatch {
    pub commits: Vec<GitGraphCommit>,
    pub done: bool,
}

/// 某次提交触及的一个文件(git_commit_files)
#[derive(Debug, Clone, Serialize)]
pub struct GitCommitFile {
    /// 仓库相对路径(重命名时为新路径)
    pub path: String,
    /// 重命名前的旧路径(仅 status = R 时有值)
    pub old_path: Option<String>,
    /// 变更类型:A 新增 / M 修改 / D 删除 / R 重命名 / T 类型变更
    pub status: String,
    /// 新增行数;二进制文件为 None(numstat 显示 -)
    pub additions: Option<u32>,
    /// 删除行数;二进制文件为 None
    pub deletions: Option<u32>,
}

/// 某次提交中单个文件的 diff(git_commit_file_diff;超长时截断)
#[derive(Debug, Clone, Serialize)]
pub struct GitCommitFileDiff {
    pub diff: String,
    /// diff 是否因超长被截断
    pub truncated: bool,
}

/// 工作区待提交的一个变更文件(git_worktree_files,提交对话框变更预览用)
#[derive(Debug, Clone, Serialize)]
pub struct GitWorktreeFile {
    /// 仓库相对路径(重命名时为新路径)
    pub path: String,
    /// 重命名前的旧路径(仅 status = R 时有值)
    pub old_path: Option<String>,
    /// 变更类型:A 新增 / M 修改 / D 删除 / R 重命名 / T 类型变更
    pub status: String,
    /// 新增行数;二进制文件为 None(numstat 显示 -)
    pub additions: Option<u32>,
    /// 删除行数;二进制文件为 None
    pub deletions: Option<u32>,
    /// 是否未跟踪文件(勾选"包含未跟踪文件"才会被提交)
    pub untracked: bool,
}

/// 仓库当前 git 用户身份(user.name / user.email,含全局配置回退)
#[derive(Debug, Clone, Serialize)]
pub struct GitUser {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<Tag>,
    pub git: Option<GitStatus>,
    /// 运行时计算:登记的目录当前是否仍存在(被移动/删除/盘符离线时为 false)
    pub path_exists: bool,
    pub archived_at: Option<i64>,
    /// 收藏时间(NULL = 未收藏;列表中收藏项目置顶,组内按收藏时间倒序)
    pub favorited_at: Option<i64>,
    /// 跟踪更新:开启后后台循环在远端有更新时自动快进拉取(无法快进即取消,不提醒)
    pub auto_pull: bool,
    /// Wiki 自动增量更新(项目级):跟踪拉取后未同步提交数达全局阈值时自动增量更新;
    /// 本地 HEAD 变化后按前端全局/项目级策略触发,与 auto_pull 相互独立
    pub wiki_auto_update: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageScript {
    pub name: String,
    pub command: String,
}

/// 一个 package.json 的 scripts 分组(monorepo 下可能有多个)
#[derive(Debug, Clone, Serialize)]
pub struct PackageScriptsGroup {
    /// package.json 所在目录的相对路径('/' 分隔),根目录为 "."
    pub dir: String,
    /// package.json 的 name 字段(可能缺失)
    pub package_name: Option<String>,
    pub scripts: Vec<PackageScript>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CustomCommand {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub command: String,
    pub description: String,
    pub icon: String,
    pub sort_order: i64,
}

/// 文件预览内容(read_file_preview):text 为 None 表示二进制文件不可预览
#[derive(Debug, Clone, Serialize)]
pub struct FilePreview {
    pub text: Option<String>,
    /// 文本是否因超过大小上限被截断
    pub truncated: bool,
}

/// 项目文件清单条目(list_project_files / search_project_files)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileEntry {
    /// 项目相对路径('/' 分隔)
    pub path: String,
    /// 是否被 .gitignore / .ignore 排除(前端灰显用)
    pub ignored: bool,
    /// 是否目录(list_project_files 逐层返回会包含目录,空目录因此可见)
    pub is_dir: bool,
}

/// 全文搜索结果中的一行命中(search_project_text)
#[derive(Debug, Clone, Serialize)]
pub struct TextSearchLine {
    /// 1-based 行号
    pub line: u32,
    /// 行内容(超长行截取首个匹配附近窗口,首尾以 … 标记)
    pub text: String,
}

/// 全文搜索结果中单个文件的命中(search_project_text)
#[derive(Debug, Clone, Serialize)]
pub struct TextSearchHit {
    /// 项目相对路径('/' 分隔)
    pub path: String,
    /// 该文件内的匹配总数
    pub count: u32,
    /// 命中行(按行号升序)
    pub lines: Vec<TextSearchLine>,
}

/// 全文搜索结果(search_project_text)
#[derive(Debug, Clone, Serialize)]
pub struct TextSearchOutcome {
    /// 命中文件(按路径排序)
    pub hits: Vec<TextSearchHit>,
    /// 是否因命中数/文件数上限被截断(前端提示仍有更多匹配)
    pub truncated: bool,
}

/// 一条可浏览器访问的端口映射:宿主机发布端口 -> 容器端口
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComposePort {
    /// 宿主机发布端口(浏览器访问入口)
    pub published: u16,
    /// 容器内目标端口
    pub target: u16,
}

/// compose 文件中的一个服务及其对外可访问的宿主机端口
#[derive(Debug, Clone, Serialize)]
pub struct ComposeService {
    pub name: String,
    /// 端口映射(按发布端口去重升序);仅含可浏览器访问的固定发布端口
    pub ports: Vec<ComposePort>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposeFile {
    /// 相对项目根的路径('/' 分隔),如 "compose.yml" 或 "deploy/app.yml"
    pub path: String,
    pub file_name: String,
    pub services: Vec<ComposeService>,
}

/// `docker compose ps` 查询到的单个服务运行状态
#[derive(Debug, Clone, Serialize)]
pub struct ComposeServiceState {
    pub name: String,
    pub running: bool,
    /// 原始状态文案,如 "Up 2 hours" / "Exited (0) 5 minutes ago"
    pub status: String,
}

/// Java 构建工具类型
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JavaBuildTool {
    Maven,
    Gradle,
}

/// 「更多操作」下拉里的一条常用命令(maven/gradle 生命周期目标)
#[derive(Debug, Clone, Serialize)]
pub struct JavaCommandAction {
    /// 前端 i18n 键(java.clean / java.package / java.install / java.test / java.build)
    pub key: String,
    pub command: String,
}

/// 一个 Spring Boot 构建文件的运行分组(monorepo 下可能有多个)。
/// 只收录构建文件声明了 spring-boot 运行插件的目录,普通 Java 项目不产出。
#[derive(Debug, Clone, Serialize)]
pub struct JavaBuildGroup {
    /// 构建文件所在目录的相对路径('/' 分隔),根目录为 "."
    pub dir: String,
    pub tool: JavaBuildTool,
    /// 运行命令应执行的工作目录的相对路径:
    /// 多模块 Maven/Gradle 子模块统一在项目根执行(见 build_run_spec)
    pub run_dir: String,
    /// 平台相关的运行命令(优先项目内 wrapper,否则用 PATH 上的 mvn/gradle)
    pub run_command: String,
    /// 常用操作(clean/package/install/test 等),与 run_command 同一执行目录
    pub more_actions: Vec<JavaCommandAction>,
}

/// 详情页资产扫描结果(scan_project_assets):单次目录遍历同时产出,
/// 避免 package scripts / compose 文件 / java 构建文件分别全量扫描
#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssets {
    pub package_scripts: Vec<PackageScriptsGroup>,
    pub compose_files: Vec<ComposeFile>,
    pub java_builds: Vec<JavaBuildGroup>,
}

/// 自动探测到的 JDK 候选(detect_jdks);install_jdk 安装成功也返回同一结构
#[derive(Debug, Clone, Serialize)]
pub struct JdkCandidate {
    /// JDK 根目录(JAVA_HOME)
    pub path: String,
    /// `java -version` 解析出的版本串,如 "17.0.2" / "1.8.0_392"
    pub version: String,
}

/// 在线安装 JDK 的发行源(list_remote_jdks / install_jdk 命令参数)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JdkVendor {
    /// Eclipse Adoptium(Temurin),元数据 api.adoptium.net
    Adoptium,
    /// Azul Zulu,元数据 api.azul.com
    Zulu,
}

/// 某安装源可在线安装的 JDK 大版本(list_remote_jdks)
#[derive(Debug, Clone, Serialize)]
pub struct RemoteJdkRelease {
    /// 主版本号(8 / 11 / 17 / 21 / 25 ...)
    pub major: u32,
    /// 该主版本当前最新的完整版本串,如 "17.0.20+8"
    pub version: String,
}

/// 工具链所属生态(detect_toolchains 输出的分组)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolchainKind {
    Rust,
    Python,
    Node,
    Dotnet,
    Git,
}

/// 版本管理器登记的一个版本(rustup 工具链 / nvm·fnm·vp 的 Node 版本 / dotnet SDK)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolchainVersion {
    /// 版本或工具链名,如 "22.11.0" / "stable-x86_64-pc-windows-msvc"
    pub name: String,
    /// 是否为当前生效的全局默认
    pub current: bool,
}

/// 该工具在当前平台/安装来源下支持的操作(设置页按钮可见性;
/// detect_toolchains 与 toolchain_op 共用同一套判定,保证按钮与实际命令一致)
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ToolchainCaps {
    pub can_install: bool,
    pub can_update: bool,
    pub can_uninstall: bool,
    /// 是否有版本管理能力(列出/切换全局版本/装卸指定版本;dotnet 只列出不可切换)
    pub can_switch: bool,
    /// 「添加版本」能否拉取远端可安装列表(list_toolchain_versions);
    /// 否(rustup 工具链名、unix nvm)时前端退化为自由输入
    pub can_list_remote: bool,
}

/// 「添加版本」的远端可装版本(list_toolchain_versions)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolchainRemoteVersion {
    pub name: String,
    /// 版本线标记文字,直接取自数据源原样展示:nvm 为表格列头
    /// (CURRENT / LTS / OLD STABLE / OLD UNSTABLE),vp 为 LTS / Current;
    /// 无从判定为 None
    pub tag: Option<String>,
}

/// 单个工具链工具的检测结果(detect_toolchains)
#[derive(Debug, Clone, Serialize)]
pub struct ToolchainStatus {
    /// CLI 名(rustup / rustc / cargo / uv / nvm / fnm / vp / dotnet / git / gh)
    pub id: String,
    pub kind: ToolchainKind,
    pub found: bool,
    /// `--version` 解析出的版本串;工具已装但输出无法解析时为 None
    pub version: Option<String>,
    /// PATH 上命中的可执行文件路径(unix nvm 是 shell 函数无二进制,为 None)
    pub path: Option<String>,
    /// 安装来源:"winget" / "rustup" / "brew" / "standalone"(决定更新/卸载命令)
    pub source: Option<String>,
    /// 版本管理器探测到的版本列表(无版本管理能力为空)
    pub versions: Vec<ToolchainVersion>,
    /// gh:当前登录用户名(`gh auth status` 解析;未登录/探测失败为 None)
    pub account: Option<String>,
    pub caps: ToolchainCaps,
}

/// 项目维度被隐藏的 UI 项
/// kind: "packageFile"(整个 package.json 分组)/ "packageScript"(分组内单条命令)/ "composeFile"
/// target_key: packageFile = 分组 dir;packageScript = "<dir>\n<name>";composeFile = 文件相对路径
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HiddenItem {
    pub kind: String,
    pub target_key: String,
}

/// 一条被标记为「常用」的命令,在托盘弹窗项目列表中可直接执行
/// kind: "packageScript" / "composeFile" / "composeService" / "customCommand"
/// target_key: packageScript = "<dir>\n<name>";composeFile = 文件相对路径;
///             composeService = "<file>\n<service>";customCommand = 命令 id
/// command: npm/自定义为完整命令;compose 类为基础前缀 `docker compose -f "..."`,动作在执行时拼接
#[derive(Debug, Clone, Serialize)]
pub struct PinnedCommand {
    pub id: i64,
    pub project_id: i64,
    pub kind: String,
    pub target_key: String,
    pub label: String,
    pub command: String,
    /// 可选工作目录:相对项目根(monorepo 子包),执行时拼接 project.path,迁移目录后仍可用
    pub cwd: Option<String>,
    /// 自定义命令的图标名(list 时 LEFT JOIN custom_commands 实时取,其他 kind 恒为 None)
    pub icon: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorKind {
    Explorer,
    Vscode,
    Cursor,
    Windsurf,
    Trae,
    Vscodium,
    Zed,
    Sublime,
    Idea,
    Webstorm,
    Goland,
    Pycharm,
    Clion,
    Rustrover,
    Terminal,
}

// ── AI 用量统计 ───────────────────────────────────────────────────────

/// ai_usage_log 的一行(明细日志)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageEntry {
    pub id: i64,
    pub created_at: i64,
    pub task_type: String,
    pub model: String,
    /// provider 未返回 usage 时为 None(不计入汇总求和,调用次数仍统计)
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    /// 缓存命中的输入 tokens(input_tokens 的子集);未返回时为 None
    pub cached_tokens: Option<i64>,
}

/// 一次用量记录的入参(id/created_at 由落库时补)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageRecord {
    pub task_type: String,
    #[serde(default)]
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub duration_ms: Option<i64>,
    pub cached_tokens: Option<i64>,
}

/// 汇总统计(get_ai_usage_summary 返回)
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageSummary {
    pub total_calls: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cached_tokens: i64,
    pub by_task: Vec<AiUsageTaskStat>,
    /// 按日本机时区分组,最近 30 天倒序
    pub by_day: Vec<AiUsageDayStat>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageTaskStat {
    pub task_type: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_tokens: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageDayStat {
    /// YYYY-MM-DD
    pub day: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cached_tokens: i64,
}
