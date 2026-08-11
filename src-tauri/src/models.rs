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

/// 本地/远程分支列表(remote 不含 origin/HEAD 这类符号引用)
#[derive(Debug, Clone, Serialize)]
pub struct GitBranches {
    pub local: Vec<String>,
    pub remote: Vec<String>,
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
}

/// `git merge` 的结果:最新状态 + 产生的合并冲突文件(为空表示无冲突)
#[derive(Debug, Clone, Serialize)]
pub struct GitMergeResult {
    pub status: GitStatus,
    pub conflicts: Vec<String>,
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

#[derive(Debug, Clone, Serialize)]
pub struct ReadmeContent {
    pub file_name: String,
    pub content: String,
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

/// 详情页资产扫描结果(scan_project_assets):单次目录遍历同时产出,
/// 避免 package scripts 与 compose 文件分别全量扫描
#[derive(Debug, Clone, Serialize)]
pub struct ProjectAssets {
    pub package_scripts: Vec<PackageScriptsGroup>,
    pub compose_files: Vec<ComposeFile>,
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
