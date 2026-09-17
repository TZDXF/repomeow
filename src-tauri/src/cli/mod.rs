//! RepoMeow CLI:主程序内置的命令行模式,面向 AI agent 与脚本提供
//! 项目管理、Wiki、语义分析、报告、标签与 AI 配置等应用管理能力(git/docker 等本机工具不包装)。
//!
//! - 首参数命中子命令(git/wiki/sem/project/report)即进入 CLI 模式,执行后退出,不启动桌面窗口;
//! - 成功结果以 JSON 写 stdout,失败以 {"code","message","detail"} JSON 写 stderr 且退出码非 0;
//! - 数据目录默认 `~/.repomeow`,可用环境变量 REPOMEOW_DATA_DIR 覆盖。

use std::env;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use crate::commands::semantic;
use crate::path_util::clean_str;

mod account_tool;
mod ai_tool;
mod file_tool;
mod pin_tool;
mod project_tool;
mod report_tool;
mod script_tool;
pub(crate) mod skills;
mod tag_tool;
#[cfg(test)]
mod tests;
mod types;
mod util;
mod wiki_tool;

use account_tool::*;
use ai_tool::*;
use file_tool::*;
use pin_tool::*;
use project_tool::*;
use report_tool::*;
use script_tool::*;
use tag_tool::*;
use types::*;
use util::*;
use wiki_tool::*;

#[derive(Debug, Parser)]
#[command(
    name = "repomeow",
    version,
    about = "RepoMeow CLI:项目管理 / Wiki / 语义分析 / 报告 / 标签 / 命令 / AI 配置(输出均为 JSON)",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 项目 Wiki 查询
    #[command(subcommand)]
    Wiki(WikiCommands),
    /// 代码语义分析
    #[command(subcommand)]
    Sem(SemCommands),
    /// 项目管理(登记/归档/开关)与数据查询
    #[command(subcommand)]
    Project(ProjectCommands),
    /// 日报/周报与报告调度
    #[command(subcommand)]
    Report(ReportCommands),
    /// 标签管理
    #[command(subcommand)]
    Tag(TagCommands),
    /// 项目自定义命令管理
    #[command(subcommand)]
    Script(ScriptCommands),
    /// 常用命令标记(托盘快捷入口)
    #[command(subcommand)]
    Pin(PinCommands),
    /// 项目条目隐藏
    #[command(subcommand)]
    Hidden(HiddenCommands),
    /// AI 接入配置、模型探测、连接测试与用量
    #[command(subcommand)]
    Ai(AiCommands),
    /// AI 提示词管理
    #[command(subcommand)]
    Prompt(PromptCommands),
    /// Git 平台账号管理(github/gitee/gitlab)
    #[command(subcommand)]
    Account(AccountCommands),
    /// 项目文件浏览/搜索/写入
    #[command(subcommand)]
    File(FileCommands),
}

#[derive(Debug, Subcommand)]
enum WikiCommands {
    /// 获取已生成 Wiki 的目录与 meta.json 元数据
    Dir {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 列出 Wiki 大纲(每页 id/标题/简介/分区/来源文件,及 stale 标记)
    Pages {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 读取 Wiki 某一页正文(页面 id 来自 pages;超长截断)
    Read {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 页面 id(来自 pages 的大纲清单)
        #[arg(long)]
        page_id: String,
    },
    /// 读取项目 Wiki 生成配置(未配置过时返回默认配置)
    ConfigGet {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 覆盖式写入项目 Wiki 生成配置(所有字段可选,缺省跟随应用默认)
    ConfigSet {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 模型引用(复合值 providerId/modelId;不传 = 跟随默认模型)
        #[arg(long)]
        model: Option<String>,
        /// 思考强度(off/minimal/low/medium/high/xhigh/max;不传 = 模型默认)
        #[arg(long)]
        thinking: Option<String>,
        /// 页面并发数(不传/0 = 跟随全局 AI 并发)
        #[arg(long)]
        concurrency: Option<usize>,
    },
    /// 删除项目已生成的 Wiki(整个 wiki 目录;不影响项目登记)
    Delete {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
}

#[derive(Debug, Subcommand)]
enum SemCommands {
    /// 按名称语义搜索代码实体(函数/类/接口/结构体等)
    Find {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
        /// 搜索关键词:实体名或其一部分
        #[arg(short, long)]
        query: String,
    },
    /// 查看实体语义上下文:源码摘要 + 按调用/引用关系扩展的相关实体
    Context {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
        /// 实体名或 entityId(形如 src/a.ts::function::run)
        #[arg(short, long)]
        entity: String,
        /// 实体所在文件的仓库相对路径(/ 分隔),重名时消歧
        #[arg(long)]
        file_path: Option<String>,
        /// 上下文预算(token 数,500-4000),缺省 2000
        #[arg(long)]
        budget: Option<u32>,
        /// 关系扩展跳数(0-3),缺省 1
        #[arg(long)]
        hops: Option<u32>,
    },
    /// 查询实体的直接调用方(callers)与引用点(refs)
    Relations {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
        /// 实体名或 entityId(含 "::" 视为 entityId)
        #[arg(short, long)]
        entity: String,
        /// 实体所在文件的仓库相对路径(/ 分隔),重名时消歧
        #[arg(long)]
        file_path: Option<String>,
    },
    /// 汇总仓库当前未提交的实体级结构化变更
    Diff {
        /// Git 仓库目录(仓库内任意路径均可)
        #[arg(short, long)]
        directory: String,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommands {
    /// 读取项目内单个文本文件的指定行区间(content 带 1-based 行号前缀)
    ReadFile {
        /// 项目目录(读取范围以该目录为根,拒绝越界与符号链接逃逸)
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 项目内相对路径(/ 分隔)
        #[arg(long)]
        path: String,
        /// 起始行(1-based),默认 1
        #[arg(long)]
        offset_line: Option<u64>,
        /// 最多返回行数,默认 400,上限 5000
        #[arg(long)]
        max_lines: Option<u64>,
    },
    /// 列出已生成的日报/周报历史(按生成时间倒序)
    Reports {
        /// 限定单个项目(RepoMeow 登记目录);缺省列出全部项目
        #[arg(short = 'p', long)]
        project_directory: Option<String>,
        /// 返回条数(1-50),默认 10
        #[arg(long)]
        limit: Option<u32>,
    },
    /// 列出项目登记的自定义命令(了解项目构建/运行方式的捷径)
    Commands {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 列出项目(默认未归档;--archived 列出已归档)
    List {
        /// 列出已归档项目
        #[arg(long)]
        archived: bool,
    },
    /// 按登记目录查询单个项目(含已归档)
    Get {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 登记项目(目录必须已存在;--name 缺省取目录名)
    Add {
        /// 项目目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 项目名称
        #[arg(long)]
        name: Option<String>,
        /// 项目描述
        #[arg(long)]
        description: Option<String>,
    },
    /// 更新项目名称/描述(缺省项保持原值)
    Update {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 新名称
        #[arg(long)]
        name: Option<String>,
        /// 新描述
        #[arg(long)]
        description: Option<String>,
    },
    /// 归档项目(数据保留,不参与后台 git 检查)
    Archive {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 恢复已归档项目
    Unarchive {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 删除项目登记(仅移除登记与关联数据,不删除磁盘目录)
    Delete {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 设置/取消收藏
    SetFavorite {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// true 收藏 / false 取消
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: bool,
    },
    /// 设置/取消「跟踪更新」(远端有更新时自动快进拉取)
    SetAutoPull {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// true 开启 / false 关闭
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: bool,
    },
    /// 设置/取消「Wiki 自动增量更新」
    SetWikiAutoUpdate {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// true 开启 / false 关闭
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ReportCommands {
    /// 为一个或多个已登记项目生成日报/周报(调用 AI 并写入报告历史,消耗 AI 额度)
    Generate {
        /// 项目目录列表(逗号分隔或重复传入;必须已登记且未归档)
        #[arg(short = 'p', long, value_delimiter = ',', required = true)]
        project_directories: Vec<String>,
        /// 报告类型:daily 日报 / weekly 周报
        #[arg(long)]
        period_type: String,
        /// 起始日期 YYYY-MM-DD;缺省 daily=今天、weekly=6 天前
        #[arg(long)]
        date_from: Option<String>,
        /// 结束日期 YYYY-MM-DD;缺省今天
        #[arg(long)]
        date_to: Option<String>,
        /// 提交作者范围:all 全部(默认)/ me 仅当前 git 用户
        #[arg(long)]
        author_mode: Option<String>,
        /// 报告语言:zh-CN(默认)/ en-US
        #[arg(long)]
        language: Option<String>,
    },
    /// 读取单条报告详情(含 Markdown 正文与提交记录)
    HistoryGet {
        /// 报告历史 id(来自 project reports)
        #[arg(long)]
        id: i64,
    },
    /// 删除单条报告历史
    HistoryDelete {
        /// 报告历史 id
        #[arg(long)]
        id: i64,
    },
    /// 列出全部报告调度
    Schedules,
    /// 从 JSON 文件全量覆盖报告调度(结构同 schedules 输出数组)
    SchedulesSave {
        /// 调度 JSON 文件路径
        #[arg(long)]
        file: String,
    },
    /// 列出系统级调度(当前仅 git_update 后台检查)
    SystemSchedules,
    /// 保存系统级调度 git_update(运行中的桌面应用需重启后生效)
    SystemScheduleSave {
        /// true 开启 / false 关闭
        #[arg(long, action = clap::ArgAction::Set)]
        enabled: bool,
        /// 检查间隔(分钟,1-1440)
        #[arg(long)]
        interval_minutes: u64,
    },
}

#[derive(Debug, Subcommand)]
enum TagCommands {
    /// 列出全部标签
    List,
    /// 创建标签
    Create {
        /// 标签名
        #[arg(long)]
        name: String,
        /// 颜色(#RGB/#RRGGBB/#RRGGBBAA,缺省 #3b82f6)
        #[arg(long)]
        color: Option<String>,
    },
    /// 更新标签
    Update {
        /// 标签 id
        #[arg(long)]
        id: i64,
        /// 标签名
        #[arg(long)]
        name: String,
        /// 颜色(缺省 #3b82f6)
        #[arg(long)]
        color: Option<String>,
    },
    /// 删除标签(同时解除各项目绑定)
    Delete {
        /// 标签 id
        #[arg(long)]
        id: i64,
    },
    /// 全量覆盖项目的标签绑定(传空列表即清空)
    SetProject {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 标签 id 列表(逗号分隔或重复传入)
        #[arg(long, value_delimiter = ',')]
        tag_ids: Vec<i64>,
    },
}

#[derive(Debug, Subcommand)]
enum ScriptCommands {
    /// 列出项目登记的自定义命令
    List {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 新建自定义命令
    Create {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 命令名(项目内唯一)
        #[arg(long)]
        name: String,
        /// 命令内容(shell)
        #[arg(long)]
        command: String,
        /// 描述
        #[arg(long)]
        description: Option<String>,
        /// 图标名(可选)
        #[arg(long)]
        icon: Option<String>,
    },
    /// 更新自定义命令(全量提交 name/command)
    Update {
        /// 命令 id
        #[arg(long)]
        id: i64,
        /// 命令名
        #[arg(long)]
        name: String,
        /// 命令内容(shell)
        #[arg(long)]
        command: String,
        /// 描述
        #[arg(long)]
        description: Option<String>,
        /// 图标名
        #[arg(long)]
        icon: Option<String>,
    },
    /// 删除自定义命令(连带移除其常用标记)
    Delete {
        /// 命令 id
        #[arg(long)]
        id: i64,
    },
}

#[derive(Debug, Subcommand)]
enum PinCommands {
    /// 列出常用命令标记(缺省列出全部项目)
    List {
        /// 限定单个项目(RepoMeow 登记目录)
        #[arg(short = 'p', long)]
        project_directory: Option<String>,
    },
    /// 标记/取消标记常用命令
    Set {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 条目类型:packageScript/composeFile/composeService/customCommand/javaBuild
        #[arg(long)]
        kind: String,
        /// 目标标识(如脚本名、compose 文件:服务名、自定义命令 id)
        #[arg(long)]
        target_key: String,
        /// true 标记 / false 取消
        #[arg(long, action = clap::ArgAction::Set)]
        pinned: bool,
        /// 展示名(标记时使用)
        #[arg(long)]
        label: Option<String>,
        /// 命令内容快照(标记时使用)
        #[arg(long)]
        command: Option<String>,
        /// 执行目录(可选)
        #[arg(long)]
        cwd: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum HiddenCommands {
    /// 列出项目的隐藏项
    List {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
    },
    /// 隐藏/取消隐藏条目
    Set {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 条目类型:packageFile/packageScript/composeFile/javaBuild
        #[arg(long)]
        kind: String,
        /// 目标标识
        #[arg(long)]
        target_key: String,
        /// true 隐藏 / false 取消隐藏
        #[arg(long, action = clap::ArgAction::Set)]
        hidden: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AiCommands {
    /// 读取 AI 接入配置(含 apiKey 明文;文件缺失时自动播种)
    ConfigGet,
    /// 从 JSON 文件全量覆盖 AI 接入配置(结构同 config-get 输出)
    ConfigSave {
        /// 配置 JSON 文件路径
        #[arg(long)]
        file: String,
    },
    /// 内置厂商目录(含预置模型清单)
    BuiltinProviders,
    /// 列出默认模型所属厂商的可用模型(访问厂商 API)
    Models,
    /// 测试默认模型连通性
    Test,
    /// AI 用量汇总(调用次数/token/耗时/按日按任务分布)
    UsageSummary,
    /// AI 用量明细日志(分页)
    UsageLog {
        /// 起始偏移,默认 0
        #[arg(long, default_value_t = 0)]
        offset: i64,
        /// 返回条数(1-200),默认 50
        #[arg(long)]
        limit: Option<i64>,
        /// 按任务类型过滤(如 report/wiki/chat/commit)
        #[arg(long)]
        task_type: Option<String>,
    },
    /// 清空 AI 用量日志
    UsageClear,
}

#[derive(Debug, Subcommand)]
enum PromptCommands {
    /// 读取用户自定义提示词(空串 = 使用内置默认)
    Get,
    /// 读取内置默认提示词模板
    Default,
    /// 从文件设置提示词;未指定项保持现状,--clear 将未指定项恢复默认
    Set {
        /// commit 提示词文件
        #[arg(long)]
        commit_file: Option<String>,
        /// report(日报)提示词文件
        #[arg(long)]
        report_file: Option<String>,
        /// report-weekly(周报)提示词文件
        #[arg(long)]
        report_weekly_file: Option<String>,
        /// 将本次未显式指定的提示词恢复默认
        #[arg(long)]
        clear: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AccountCommands {
    /// 列出全部 Git 平台账号(token 只回显预览)
    List,
    /// 绑定账号(先调平台 API 验证 token,成功才落库)
    Add {
        /// 平台:github / gitee / gitlab
        #[arg(long)]
        provider: String,
        /// 账号备注名
        #[arg(long)]
        label: String,
        /// 访问令牌
        #[arg(long)]
        token: String,
        /// GitLab 实例地址(gitlab 必填,如 https://gitlab.example.com)
        #[arg(long)]
        base_url: Option<String>,
    },
    /// 更新账号;--token 缺省保留原 token,token/实例地址变化时重新验证
    Update {
        /// 账号 id
        #[arg(long)]
        id: i64,
        /// 账号备注名
        #[arg(long)]
        label: String,
        /// GitLab 实例地址(仅 gitlab)
        #[arg(long)]
        base_url: Option<String>,
        /// 新访问令牌(缺省保留)
        #[arg(long)]
        token: Option<String>,
    },
    /// 删除账号
    Remove {
        /// 账号 id
        #[arg(long)]
        id: i64,
    },
}

#[derive(Debug, Subcommand)]
enum FileCommands {
    /// 列出项目目录下一层条目(文件/文件夹)
    List {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 项目内子目录(缺省根目录)
        #[arg(long)]
        dir: Option<String>,
    },
    /// 按文件名模糊搜索项目文件
    Search {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 搜索关键词
        #[arg(short, long)]
        query: String,
        /// 返回条数上限
        #[arg(long)]
        limit: Option<u32>,
    },
    /// 全文搜索项目文本内容
    SearchText {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 搜索内容
        #[arg(short, long)]
        query: String,
        /// 区分大小写
        #[arg(long)]
        case_sensitive: bool,
        /// 全词匹配
        #[arg(long)]
        whole_word: bool,
        /// 正则表达式
        #[arg(long)]
        regex: bool,
        /// 包含 glob(如 src/**、*.ts,逗号分隔)
        #[arg(long)]
        include: Option<String>,
        /// 排除 glob
        #[arg(long)]
        exclude: Option<String>,
    },
    /// 写入项目内文本文件(创建或覆盖;上限 512KB,父目录必须已存在)
    Save {
        /// RepoMeow 中项目登记使用的目录
        #[arg(short = 'p', long)]
        project_directory: String,
        /// 项目内相对路径(/ 分隔)
        #[arg(long)]
        path: String,
        /// 内联文本内容(与 --content-file 二选一)
        #[arg(long)]
        content: Option<String>,
        /// 从文件读入内容(与 --content 二选一)
        #[arg(long)]
        content_file: Option<String>,
    },
}

/// CLI 首参数白名单:命中即进入 CLI 模式。
fn is_cli_head(arg: &str) -> bool {
    matches!(
        arg,
        "git"
            | "wiki"
            | "sem"
            | "project"
            | "report"
            | "tag"
            | "script"
            | "pin"
            | "hidden"
            | "ai"
            | "prompt"
            | "account"
            | "file"
            | "docker"
            | "help"
            | "-h"
            | "--help"
            | "-V"
            | "--version"
    )
}

/// 判断是否应以 CLI 模式运行(在 Tauri 初始化前调用)。
pub fn is_cli_mode() -> bool {
    env::args_os()
        .nth(1)
        .and_then(|arg| arg.into_string().ok())
        .is_some_and(|arg| is_cli_head(&arg))
}

/// 以 CLI 模式执行并返回进程退出码。
pub fn run_cli_blocking() -> i32 {
    #[cfg(windows)]
    attach_parent_console();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "{}",
                json!({"code": "runtime_init_failed", "message": "初始化 CLI 运行时失败", "detail": error.to_string()})
            );
            return 1;
        }
    };
    runtime.block_on(run_cli())
}

/// release 为 windows_subsystem="windows" 的 GUI 进程,默认无控制台;
/// 附加到父进程控制台使终端内可见输出(输出被管道重定向时本就可写,不受影响)。
#[cfg(windows)]
fn attach_parent_console() {
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

/// 帮助与版本保留文本输出;参数解析失败遵循 CLI 的 JSON 错误协议。
fn format_parse_error(error: clap::Error) -> (i32, String) {
    let exit_code = error.exit_code();
    let output = if exit_code == 0 {
        error.to_string()
    } else {
        ToolFailure::new("invalid_arguments", "CLI 参数无效")
            .with_detail(error.to_string())
            .to_json()
            .to_string()
    };
    (exit_code, output)
}

async fn run_cli() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let (exit_code, output) = format_parse_error(error);
            if exit_code == 0 {
                print!("{output}");
            } else {
                eprintln!("{output}");
            }
            return exit_code;
        }
    };
    match dispatch(cli.command).await {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(failure) => {
            eprintln!("{}", failure.to_json());
            1
        }
    }
}

/// 同步工具实现(子进程/文件 IO)统一丢到 blocking 线程池执行。
async fn blocking_tool<F>(fail_message: &'static str, op: F) -> Result<Value, ToolFailure>
where
    F: FnOnce() -> Result<Value, ToolFailure> + Send + 'static,
{
    tokio::task::spawn_blocking(op).await.map_err(|error| {
        ToolFailure::new("task_failed", fail_message).with_detail(error.to_string())
    })?
}

async fn dispatch(command: Commands) -> Result<Value, ToolFailure> {
    match command {
        Commands::Wiki(WikiCommands::Dir { project_directory }) => Ok(json!(
            get_wiki_directory_impl(GetWikiDirectoryInput { project_directory }, None)?
        )),
        Commands::Wiki(WikiCommands::Pages { project_directory }) => Ok(json!(
            list_wiki_pages_impl(ProjectDirectoryInput { project_directory }, None)?
        )),
        Commands::Wiki(WikiCommands::Read {
            project_directory,
            page_id,
        }) => Ok(json!(read_wiki_page_impl(
            ReadWikiPageInput {
                project_directory,
                page_id,
            },
            None
        )?)),
        Commands::Sem(SemCommands::Find { directory, query }) => {
            let query = query.trim().to_string();
            if query.is_empty() {
                return Err(ToolFailure::new("invalid_query", "搜索关键词不能为空"));
            }
            let result = semantic::cli_semantic_find_entities(clean_str(&directory), query)
                .await
                .map_err(|error| ToolFailure::from_app("语义搜索失败", error))?;
            Ok(json!(result))
        }
        Commands::Sem(SemCommands::Context {
            directory,
            entity,
            file_path,
            budget,
            hops,
        }) => {
            let Some((entity_id, entity_name)) = split_entity_token(&entity) else {
                return Err(ToolFailure::new("invalid_entity", "实体名不能为空"));
            };
            let result = semantic::cli_semantic_entity_context(
                clean_str(&directory),
                entity_id,
                entity_name,
                file_path,
                budget.map(|value| value as usize),
                hops.map(|value| value as usize),
            )
            .await
            .map_err(|error| ToolFailure::from_app("读取实体上下文失败", error))?;
            Ok(json!(result))
        }
        Commands::Sem(SemCommands::Relations {
            directory,
            entity,
            file_path,
        }) => {
            let Some((entity_id, entity_name)) = split_entity_token(&entity) else {
                return Err(ToolFailure::new("invalid_entity", "实体名不能为空"));
            };
            let (callers, refs) = semantic::cli_semantic_entity_relations(
                clean_str(&directory),
                entity_id,
                entity_name,
                file_path,
            )
            .await
            .map_err(|error| ToolFailure::from_app("查询实体关系失败", error))?;
            Ok(json!({ "callers": callers, "refs": refs }))
        }
        Commands::Sem(SemCommands::Diff { directory }) => {
            let result = semantic::cli_semantic_worktree_diff(clean_str(&directory))
                .await
                .map_err(|error| ToolFailure::from_app("读取未提交变更失败", error))?;
            Ok(json!(result))
        }
        Commands::Project(ProjectCommands::ReadFile {
            project_directory,
            path,
            offset_line,
            max_lines,
        }) => Ok(json!(read_project_file_impl(ReadProjectFileInput {
            project_directory,
            path,
            offset_line,
            max_lines,
        })?)),
        Commands::Project(ProjectCommands::Reports {
            project_directory,
            limit,
        }) => list_reports_impl(
            ListReportsInput {
                project_directory,
                limit,
            },
            None,
        ),
        Commands::Project(ProjectCommands::Commands { project_directory }) => {
            list_custom_commands_impl(ProjectDirectoryInput { project_directory }, None)
        }
        Commands::Report(ReportCommands::Generate {
            project_directories,
            period_type,
            date_from,
            date_to,
            author_mode,
            language,
        }) => {
            generate_report_impl(
                GenerateReportInput {
                    project_directories,
                    period_type,
                    date_from,
                    date_to,
                    author_mode,
                    language,
                },
                None,
            )
            .await
        }
        // ── Wiki 管理 ───────────────────────────────────────────────
        Commands::Wiki(WikiCommands::ConfigGet { project_directory }) => {
            wiki_config_get_impl(&project_directory, None)
        }
        Commands::Wiki(WikiCommands::ConfigSet {
            project_directory,
            model,
            thinking,
            concurrency,
        }) => wiki_config_set_impl(&project_directory, model, thinking, concurrency, None),
        Commands::Wiki(WikiCommands::Delete { project_directory }) => {
            delete_wiki_impl(&project_directory, None)
        }
        // ── 项目登记管理 ─────────────────────────────────────────────
        Commands::Project(ProjectCommands::List { archived }) => list_projects_impl(archived, None),
        Commands::Project(ProjectCommands::Get { project_directory }) => {
            get_project_impl(ProjectDirectoryInput { project_directory }, None)
        }
        Commands::Project(ProjectCommands::Add {
            project_directory,
            name,
            description,
        }) => add_project_impl(&project_directory, name, description, None),
        Commands::Project(ProjectCommands::Update {
            project_directory,
            name,
            description,
        }) => update_project_impl(&project_directory, name, description, None),
        Commands::Project(ProjectCommands::Archive { project_directory }) => {
            set_project_archived_impl(&project_directory, true, None)
        }
        Commands::Project(ProjectCommands::Unarchive { project_directory }) => {
            set_project_archived_impl(&project_directory, false, None)
        }
        Commands::Project(ProjectCommands::Delete { project_directory }) => {
            delete_project_impl(&project_directory, None)
        }
        Commands::Project(ProjectCommands::SetFavorite {
            project_directory,
            enabled,
        }) => set_project_flag_impl(&project_directory, "favorite", enabled, None),
        Commands::Project(ProjectCommands::SetAutoPull {
            project_directory,
            enabled,
        }) => set_project_flag_impl(&project_directory, "autoPull", enabled, None),
        Commands::Project(ProjectCommands::SetWikiAutoUpdate {
            project_directory,
            enabled,
        }) => set_project_flag_impl(&project_directory, "wikiAutoUpdate", enabled, None),
        // ── 报告历史与调度 ───────────────────────────────────────────
        Commands::Report(ReportCommands::HistoryGet { id }) => get_report_impl(id, None),
        Commands::Report(ReportCommands::HistoryDelete { id }) => delete_report_impl(id, None),
        Commands::Report(ReportCommands::Schedules) => list_schedules_impl(None),
        Commands::Report(ReportCommands::SchedulesSave { file }) => {
            save_schedules_impl(&file, None)
        }
        Commands::Report(ReportCommands::SystemSchedules) => list_system_schedules_impl(None),
        Commands::Report(ReportCommands::SystemScheduleSave {
            enabled,
            interval_minutes,
        }) => save_system_schedule_impl(enabled, interval_minutes, None),
        // ── 标签 ────────────────────────────────────────────────────
        Commands::Tag(TagCommands::List) => list_tags_impl(None),
        Commands::Tag(TagCommands::Create { name, color }) => {
            create_tag_impl(&name, color.as_deref(), None)
        }
        Commands::Tag(TagCommands::Update { id, name, color }) => {
            update_tag_impl(id, &name, color.as_deref(), None)
        }
        Commands::Tag(TagCommands::Delete { id }) => delete_tag_impl(id, None),
        Commands::Tag(TagCommands::SetProject {
            project_directory,
            tag_ids,
        }) => set_project_tags_impl(&project_directory, tag_ids, None),
        // ── 自定义命令 ───────────────────────────────────────────────
        Commands::Script(ScriptCommands::List { project_directory }) => {
            list_custom_commands_impl(ProjectDirectoryInput { project_directory }, None)
        }
        Commands::Script(ScriptCommands::Create {
            project_directory,
            name,
            command,
            description,
            icon,
        }) => create_command_impl(
            &project_directory,
            &name,
            &command,
            description.as_deref(),
            icon.as_deref(),
            None,
        ),
        Commands::Script(ScriptCommands::Update {
            id,
            name,
            command,
            description,
            icon,
        }) => update_command_impl(
            id,
            &name,
            &command,
            description.as_deref(),
            icon.as_deref(),
            None,
        ),
        Commands::Script(ScriptCommands::Delete { id }) => delete_command_impl(id, None),
        // ── 常用命令标记 / 隐藏项 ─────────────────────────────────────
        Commands::Pin(PinCommands::List { project_directory }) => {
            list_pins_impl(project_directory.as_deref(), None)
        }
        Commands::Pin(PinCommands::Set {
            project_directory,
            kind,
            target_key,
            pinned,
            label,
            command,
            cwd,
        }) => set_pin_impl(
            &project_directory,
            &kind,
            &target_key,
            pinned,
            label.as_deref(),
            command.as_deref(),
            cwd.as_deref(),
            None,
        ),
        Commands::Hidden(HiddenCommands::List { project_directory }) => {
            list_hidden_impl(&project_directory, None)
        }
        Commands::Hidden(HiddenCommands::Set {
            project_directory,
            kind,
            target_key,
            hidden,
        }) => set_hidden_impl(&project_directory, &kind, &target_key, hidden, None),
        // ── AI 配置 / 用量 / 提示词 ───────────────────────────────────
        Commands::Ai(AiCommands::ConfigGet) => ai_config_get_impl(None),
        Commands::Ai(AiCommands::ConfigSave { file }) => ai_config_save_impl(&file, None),
        Commands::Ai(AiCommands::BuiltinProviders) => ai_builtin_providers_impl(),
        Commands::Ai(AiCommands::Models) => ai_models_impl(None).await,
        Commands::Ai(AiCommands::Test) => ai_test_impl(None).await,
        Commands::Ai(AiCommands::UsageSummary) => ai_usage_summary_impl(None),
        Commands::Ai(AiCommands::UsageLog {
            offset,
            limit,
            task_type,
        }) => ai_usage_log_impl(
            offset,
            limit.unwrap_or(50).clamp(1, 200),
            task_type.as_deref(),
            None,
        ),
        Commands::Ai(AiCommands::UsageClear) => ai_usage_clear_impl(None),
        Commands::Prompt(PromptCommands::Get) => prompts_get_impl(None),
        Commands::Prompt(PromptCommands::Default) => prompts_default_impl(),
        Commands::Prompt(PromptCommands::Set {
            commit_file,
            report_file,
            report_weekly_file,
            clear,
        }) => prompts_set_impl(
            commit_file.as_deref(),
            report_file.as_deref(),
            report_weekly_file.as_deref(),
            clear,
            None,
        ),
        // ── Git 账号 ─────────────────────────────────────────────────
        Commands::Account(AccountCommands::List) => list_accounts_impl(None),
        Commands::Account(AccountCommands::Add {
            provider,
            label,
            token,
            base_url,
        }) => add_account_impl(&provider, &label, base_url.as_deref(), &token, None).await,
        Commands::Account(AccountCommands::Update {
            id,
            label,
            base_url,
            token,
        }) => update_account_impl(id, &label, base_url.as_deref(), token.as_deref(), None).await,
        Commands::Account(AccountCommands::Remove { id }) => remove_account_impl(id, None),
        // ── 项目文件 / Docker ─────────────────────────────────────────
        Commands::File(FileCommands::List {
            project_directory,
            dir,
        }) => {
            blocking_tool("列出项目文件任务失败", move || {
                list_files_impl(&project_directory, dir.as_deref())
            })
            .await
        }
        Commands::File(FileCommands::Search {
            project_directory,
            query,
            limit,
        }) => {
            blocking_tool("搜索项目文件任务失败", move || {
                search_files_impl(&project_directory, &query, limit)
            })
            .await
        }
        Commands::File(FileCommands::SearchText {
            project_directory,
            query,
            case_sensitive,
            whole_word,
            regex,
            include,
            exclude,
        }) => {
            blocking_tool("搜索项目文本任务失败", move || {
                search_text_impl(
                    &project_directory,
                    &query,
                    case_sensitive,
                    whole_word,
                    regex,
                    include.as_deref(),
                    exclude.as_deref(),
                )
            })
            .await
        }
        Commands::File(FileCommands::Save {
            project_directory,
            path,
            content,
            content_file,
        }) => save_file_impl(
            &project_directory,
            &path,
            content.as_deref(),
            content_file.as_deref(),
        ),
    }
}
