//! wiki agent 后端:经 ACP(Agent Client Protocol,JSON-RPC/NDJSON over stdio)
//! 调用用户本地已认证的 coding agent CLI。
//!
//! 协议编解码与连接驱动由官方 agent-client-protocol crate 承担;本模块负责:
//! - 精选 agent 清单与安装探测(agent_list;条目数据摘自 ACP registry,2026-08)
//! - 会话生命周期:acp_start(spawn + initialize + session/new,可选应用模型/思考强度
//!   配置)/ acp_prompt(Channel 流式推送正文块与活动行)/ acp_cancel(session/cancel +
//!   超时杀进程树)/ acp_test(设置页「测试」与自动获取模型清单:建会话读
//!   config_options/modes 后即收尾)
//! - 模型/思考强度:session/new 响应的 config_options(category=model/thought_level)
//!   经 session/set_config_option 应用;agent 未上报 config_options 时回退旧式
//!   modes(session/set_mode)
//! - 进程管理:自行 spawn 以持有 PID(Windows 用 taskkill /T /F 杀整棵进程树,
//!   覆盖 npx.cmd→cmd→node 包装链;crate 内置 ChildGuard 在 Windows 只杀直接子进程)
//! - 客户端回调:fs/read_text_file(限定会话 cwd 内,防越界读)+ 权限请求
//!   (headless 场景固定自动选第一个选项,并作为活动行告知前端;不暴露用户决策)
//!
//! 生成编排在前端(stores/wiki.ts + lib/wiki-generator.ts):本模块只提供
//! 「一次 prompt → 流式事件 + 最终全文」的通用会话能力,不感知 wiki 语义。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, FileSystemCapabilities, Implementation,
    InitializeRequest, NewSessionRequest, NewSessionResponse, PromptRequest, ReadTextFileRequest,
    ReadTextFileResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelectOptions, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest,
    StopReason, TextContent, Usage,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot, Notify};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::time_util::now_ts_nanos;

/// initialize 握手超时(npx 首次下载适配器包可能较慢)
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// acp_start 等待握手完成的总超时(略宽于 INIT_TIMEOUT,容纳 spawn 与会话创建)
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(75);
/// cancel 后等待 agent 自行收尾的宽限,超时强杀进程树
const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// 应用模型/思考强度配置(set_config_option/set_mode)的单次超时
const CONFIG_TIMEOUT: Duration = Duration::from_secs(10);
/// stderr 尾部保留量(诊断 agent 报错用)
const STDERR_TAIL_MAX: usize = 8 * 1024;
/// fs/read_text_file 单次返回上限
const READ_FILE_MAX: usize = 256 * 1024;

// ── 精选 agent 清单(摘自 https://cdn.agentclientprotocol.com/registry)──────

/// 分发方式:npx 包(需 Node)或原生二进制
enum AgentKind {
    Npx {
        pkg: &'static str,
        args: &'static [&'static str],
    },
    Binary {
        cmd: &'static str,
        args: &'static [&'static str],
    },
}

struct AgentDef {
    id: &'static str,
    name: &'static str,
    kind: AgentKind,
    /// 未安装/未登录时的指引(设置页展示)
    login_hint: &'static str,
}

static AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        name: "Claude Code",
        kind: AgentKind::Npx {
            pkg: "@agentclientprotocol/claude-agent-acp",
            args: &[],
        },
        login_hint: "终端运行 claude 并按提示登录(Anthropic 账号)",
    },
    AgentDef {
        id: "codex",
        name: "Codex",
        kind: AgentKind::Npx {
            pkg: "@agentclientprotocol/codex-acp",
            args: &[],
        },
        login_hint: "终端运行 codex login 登录(OpenAI 账号)",
    },
    AgentDef {
        id: "gemini",
        name: "Gemini CLI",
        kind: AgentKind::Npx {
            pkg: "@google/gemini-cli",
            args: &["--acp"],
        },
        login_hint: "终端运行 gemini 并按提示登录(Google 账号)",
    },
    AgentDef {
        id: "copilot",
        name: "GitHub Copilot",
        kind: AgentKind::Npx {
            pkg: "@github/copilot",
            args: &["--acp"],
        },
        login_hint: "终端运行 copilot 并按提示登录(GitHub 账号)",
    },
    AgentDef {
        id: "grok",
        name: "Grok Build",
        kind: AgentKind::Npx {
            pkg: "@xai-official/grok",
            args: &["agent", "stdio"],
        },
        login_hint: "终端运行 grok 并配置 xAI 凭证",
    },
    AgentDef {
        id: "qwen",
        name: "Qwen Code",
        kind: AgentKind::Npx {
            pkg: "@qwen-code/qwen-code",
            args: &["--acp"],
        },
        login_hint: "终端运行 qwen 并按提示登录",
    },
    AgentDef {
        id: "cline",
        name: "Cline",
        kind: AgentKind::Npx {
            pkg: "cline",
            args: &["--acp"],
        },
        login_hint: "终端运行 cline 并按提示登录",
    },
    AgentDef {
        id: "glm",
        name: "GLM",
        kind: AgentKind::Npx {
            pkg: "glm-acp-agent",
            args: &[],
        },
        login_hint: "配置 GLM Coding Plan 的 API Key 后使用(见 glm-acp-agent 文档)",
    },
    AgentDef {
        id: "pi",
        name: "Pi",
        kind: AgentKind::Npx {
            pkg: "pi-acp",
            args: &[],
        },
        login_hint: "先安装 pi(社区适配器 pi-acp,功能受限:无权限请求/图像)",
    },
    AgentDef {
        id: "opencode",
        name: "OpenCode",
        kind: AgentKind::Binary {
            cmd: "opencode",
            args: &["acp"],
        },
        login_hint: "安装 opencode 并完成登录(opencode.ai)",
    },
    AgentDef {
        id: "goose",
        name: "goose",
        kind: AgentKind::Binary {
            cmd: "goose",
            args: &["acp"],
        },
        login_hint: "安装 goose 并完成登录(block/goose)",
    },
    AgentDef {
        id: "cursor",
        name: "Cursor",
        kind: AgentKind::Binary {
            cmd: "cursor-agent",
            args: &["acp"],
        },
        login_hint: "安装 cursor-agent(Cursor 付费计划)",
    },
    AgentDef {
        id: "kimi",
        name: "Kimi CLI",
        kind: AgentKind::Binary {
            cmd: "kimi",
            args: &["acp"],
        },
        login_hint: "安装 Kimi CLI 并完成登录(Moonshot 账号)",
    },
];

fn agent_kind_str(kind: &AgentKind) -> &'static str {
    match kind {
        AgentKind::Npx { .. } => "npx",
        AgentKind::Binary { .. } => "binary",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    installed: bool,
    /// 探测到的可执行路径(npx 类为 npx 路径);未安装为 None
    detail: Option<String>,
    login_hint: &'static str,
}

/// 精选 agent 清单 + 安装探测(npx 类探测 node/npx,二进制类探测命令本身)
#[tauri::command]
pub fn agent_list() -> AppResult<Vec<AgentInfo>> {
    Ok(AGENTS
        .iter()
        .map(|def| {
            let (installed, detail) = match &def.kind {
                AgentKind::Npx { .. } => match (which::which("node"), which::which("npx")) {
                    (Ok(_), Ok(p)) => (true, Some(p.display().to_string())),
                    _ => (false, None),
                },
                AgentKind::Binary { cmd, .. } => match which::which(cmd) {
                    Ok(p) => (true, Some(p.display().to_string())),
                    Err(_) => (false, None),
                },
            };
            AgentInfo {
                id: def.id,
                name: def.name,
                kind: agent_kind_str(&def.kind),
                installed,
                detail,
                login_hint: def.login_hint,
            }
        })
        .collect())
}

// ── 会话作业与注册表 ────────────────────────────────────────────────────────

/// 前端流式事件:正文块(text 为累积全文,对齐内置内核的 onPartial 语义)或活动行
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AcpEvent {
    Chunk { text: String },
    Activity { text: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpStartResult {
    pub(crate) run_id: String,
    pub(crate) agent_name: String,
    /// agent 上报的会话配置选项(模型/思考强度等下拉),未上报为空
    config_options: Vec<AcpConfigOptionInfo>,
    /// 旧式 modes(无 config_options 的 agent 用它选模型档位),未上报为空
    modes: Vec<AcpModeInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTestResult {
    agent_name: String,
    config_options: Vec<AcpConfigOptionInfo>,
    modes: Vec<AcpModeInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptResult {
    stop_reason: String,
    pub(crate) text: String,
    /// 本次 prompt 的 token 用量(ACP unstable 字段;agent 未上报为 None)
    pub(crate) usage: Option<AcpTokenUsage>,
}

/// 一次 prompt 的 token 用量(ACP PromptResponse.usage,unstable 字段)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTokenUsage {
    pub(crate) total_tokens: u64,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    thought_tokens: Option<u64>,
    pub(crate) cached_read_tokens: Option<u64>,
    cached_write_tokens: Option<u64>,
}

impl From<Usage> for AcpTokenUsage {
    fn from(u: Usage) -> Self {
        Self {
            total_tokens: u.total_tokens,
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            thought_tokens: u.thought_tokens,
            cached_read_tokens: u.cached_read_tokens,
            cached_write_tokens: u.cached_write_tokens,
        }
    }
}

/// 会话配置下拉的一个可选项
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigChoice {
    id: String,
    name: String,
}

/// session/new 上报的会话配置选项(select 类;boolean 类不暴露,本项目用不上)
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOptionInfo {
    id: String,
    name: String,
    /// 语义类别:"model" / "thought_level" / "mode" / "model_config" / 其他原样
    category: Option<String>,
    /// 当前选中值 id
    current: Option<String>,
    /// 下拉可选项(分组选项已拍平)
    choices: Vec<AcpConfigChoice>,
}

/// 旧式 mode(等价于 agent 的模型/档位选择)
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpModeInfo {
    id: String,
    name: String,
}

enum JobMsg {
    Prompt {
        prompt: String,
        sender: AcpEventSender,
        done: oneshot::Sender<Result<AcpPromptResult, AppError>>,
    },
}

enum SessionMode {
    /// 正常生成:cwd 即会话工作目录(agent 的探索范围)
    Generate { cwd: PathBuf },
    /// 设置页测试/获取模型清单:建会话读完上报即收尾
    Test,
}

/// 握手结果:经 oneshot 从驱动任务传回 run_session
struct AcpHandshake {
    agent_name: String,
    config_options: Vec<AcpConfigOptionInfo>,
    modes: Vec<AcpModeInfo>,
}

struct AgentSession {
    pid: u32,
    job_tx: mpsc::UnboundedSender<JobMsg>,
    cancel: Arc<Notify>,
}

static AGENT_JOBS: OnceLock<Mutex<HashMap<String, AgentSession>>> = OnceLock::new();

fn agent_jobs() -> &'static Mutex<HashMap<String, AgentSession>> {
    AGENT_JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 运行中的 agent 子进程 PID,供应用退出钩子按 PID 杀进程树
static AGENT_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

fn agent_pids() -> &'static Mutex<HashSet<u32>> {
    AGENT_PIDS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 当前 prompt 的流式汇聚点:通知回调写入(累积正文 + Channel 推送),驱动循环读取
struct PromptSink {
    sender: AcpEventSender,
    text: String,
}

#[derive(Clone)]
pub(crate) struct AcpEventSender(Arc<dyn Fn(AcpEvent) + Send + Sync>);

impl AcpEventSender {
    pub(crate) fn new(send: impl Fn(AcpEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    fn send(&self, event: AcpEvent) {
        (self.0)(event);
    }
}

type SharedSink = Arc<Mutex<Option<PromptSink>>>;

// ── 命令 ────────────────────────────────────────────────────────────────────

/// 启动 agent 会话:spawn → initialize(V1) → session/new,握手完成即返回。
/// `agent_id` 与 `custom_command` 二选一;`cwd` 为会话工作目录(限制 fs 读取范围)。
/// `model`/`thinking` 为用户在设置页选择的模型/思考强度(来自 agent 上报的
/// config_options 或 modes 的 id),会话创建后应用;不在可选列表内则忽略并记日志。
#[tauri::command]
pub async fn acp_start(
    agent_id: Option<String>,
    custom_command: Option<String>,
    cwd: String,
    model: Option<String>,
    thinking: Option<String>,
) -> AppResult<AcpStartResult> {
    run_session(
        agent_id,
        custom_command,
        SessionMode::Generate {
            cwd: PathBuf::from(&cwd),
        },
        model,
        thinking,
        cwd,
    )
    .await
}

/// 设置页「测试」/自动获取模型清单:spawn + initialize + session/new(TEMP 目录),
/// 返回 agent 名称与其上报的 config_options/modes,随即收尾进程
#[tauri::command]
pub async fn acp_test(
    agent_id: Option<String>,
    custom_command: Option<String>,
) -> AppResult<AcpTestResult> {
    let cwd = std::env::temp_dir().display().to_string();
    let result = run_session(agent_id, custom_command, SessionMode::Test, None, None, cwd).await?;
    Ok(AcpTestResult {
        agent_name: result.agent_name,
        config_options: result.config_options,
        modes: result.modes,
    })
}

/// 发送一次 prompt 并流式等待完成。事件经 `on_event` Channel 推送
/// (Chunk 为累积正文、Activity 为工具调用/权限决策行),最终结果经返回值给出。
#[tauri::command]
pub async fn acp_prompt(
    run_id: String,
    prompt: String,
    on_event: Channel<AcpEvent>,
) -> AppResult<AcpPromptResult> {
    let sender = AcpEventSender::new(move |event| {
        let _ = on_event.send(event);
    });
    acp_prompt_with(run_id, prompt, sender).await
}

pub(crate) async fn acp_prompt_with(
    run_id: String,
    prompt: String,
    sender: AcpEventSender,
) -> AppResult<AcpPromptResult> {
    let (done_tx, done_rx) = oneshot::channel();
    {
        let jobs = agent_jobs().lock().unwrap();
        let session = jobs
            .get(&run_id)
            .ok_or_else(|| AppError::coded(ErrorCode::AgentPromptFailed, "会话不存在或已结束"))?;
        session
            .job_tx
            .send(JobMsg::Prompt {
                prompt,
                sender,
                done: done_tx,
            })
            .map_err(|_| AppError::coded(ErrorCode::AgentPromptFailed, "会话已结束"))?;
    }
    match done_rx.await {
        Ok(res) => res,
        Err(_) => Err(AppError::coded(
            ErrorCode::AgentPromptFailed,
            "会话在生成中中断(agent 进程退出)",
        )),
    }
}

/// 取消会话:发 session/cancel;宽限后仍未收尾则按 PID 杀进程树(覆盖 npx 包装链)
#[tauri::command]
pub fn acp_cancel(run_id: String) -> AppResult<()> {
    let pid = {
        let jobs = agent_jobs().lock().unwrap();
        let session = jobs
            .get(&run_id)
            .ok_or_else(|| AppError::coded(ErrorCode::AgentPromptFailed, "会话不存在或已结束"))?;
        session.cancel.notify_one();
        session.pid
    };
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CANCEL_GRACE).await;
        // 仍在注册表 = agent 未自行收尾(agent_jobs 由驱动任务在结束时移除)
        if agent_jobs().lock().unwrap().contains_key(&run_id) {
            kill_agent_pid(pid);
        }
    });
    Ok(())
}

/// 应用退出收尾:按 PID 杀所有仍在运行的 agent 进程树。由 lib.rs 的 Exit 钩子调用。
pub fn cleanup_on_exit() {
    let pids: Vec<u32> = agent_pids().lock().unwrap().drain().collect();
    for pid in pids {
        kill_agent_pid(pid);
    }
}

// ── 会话实现 ────────────────────────────────────────────────────────────────

/// spawn + 握手 + 注册;握手失败时清理并返回错误
async fn run_session(
    agent_id: Option<String>,
    custom_command: Option<String>,
    mode: SessionMode,
    model: Option<String>,
    thinking: Option<String>,
    fs_root: String,
) -> AppResult<AcpStartResult> {
    let (program, args, display_name) = resolve_spawn(agent_id, custom_command)?;

    // 经 std Command 组好再转 async(std 提供 unix process_group;async 提供 windows creation_flags)
    let mut std_cmd = std::process::Command::new(&program);
    std_cmd
        .args(&args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 独立进程组:会话结束时杀整组,避免 npx/uvx 包装器遗留孙进程
        std_cmd.process_group(0);
    }
    let mut cmd = async_process::Command::from(std_cmd);
    // 关键:async-process 的 From<std Command> 不携带「已配置管道」内部标记,
    // spawn 时它会把未置位的流强制覆盖为 Stdio::inherit(),导致 child.stdin 为 None
    // (报「无法获取 stdin 管道」)。必须用它自己的 setter 再设一次以置位标记。
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use async_process::windows::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::coded(ErrorCode::AgentSpawnFailed, format!("{program:?}: {e}")))?;

    let pid = child.id();
    let child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::coded(ErrorCode::AgentSpawnFailed, "无法获取 stdin 管道"))?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::coded(ErrorCode::AgentSpawnFailed, "无法获取 stdout 管道"))?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::coded(ErrorCode::AgentSpawnFailed, "无法获取 stderr 管道"))?;

    let run_id = format!("acp-{}", now_ts_nanos());
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(Notify::new());
    let sink: SharedSink = Arc::new(Mutex::new(None));
    let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (hs_tx, hs_rx) = oneshot::channel::<Result<AcpHandshake, AppError>>();

    agent_pids().lock().unwrap().insert(pid);

    // stderr 尾部采集(诊断 agent 启动/登录失败;限长防管道写满阻塞子进程)
    {
        let tail = stderr_tail.clone();
        tauri::async_runtime::spawn(async move {
            use futures::AsyncReadExt;
            let mut reader = child_stderr;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut t = tail.lock().unwrap();
                        t.extend_from_slice(&buf[..n]);
                        let len = t.len();
                        if len > STDERR_TAIL_MAX {
                            t.drain(..len - STDERR_TAIL_MAX);
                        }
                    }
                }
            }
        });
    }

    let fs_root = PathBuf::from(&fs_root);

    // 驱动任务:协议连接 + 握手 + prompt 循环;结束即清理注册表并杀进程
    {
        let run_id = run_id.clone();
        let cancel = cancel.clone();
        let sink = sink.clone();
        let stderr_tail = stderr_tail.clone();
        tauri::async_runtime::spawn(async move {
            let handshake_err = |e: agent_client_protocol::Error, tail: &Arc<Mutex<Vec<u8>>>| {
                AppError::coded(
                    ErrorCode::AgentHandshakeFailed,
                    format!("{e}{}", tail_text(tail)),
                )
            };
            let result = Client
                .builder()
                .on_receive_notification(
                    {
                        let sink = sink.clone();
                        async move |n: SessionNotification, _cx| {
                            route_session_update(&sink, n.update);
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    {
                        let sink = sink.clone();
                        async move |req: RequestPermissionRequest, responder, _cx| {
                            // headless 生成场景:固定自动放行第一个选项(本机自己的仓库
                            // + 用户自己认证的 agent),决策作为活动行告知前端
                            let title = req.tool_call.fields.title.clone().unwrap_or_default();
                            push_activity(&sink, format!("已允许: {title}"));
                            let outcome = req
                                .options
                                .first()
                                .map(|o| {
                                    RequestPermissionOutcome::Selected(
                                        SelectedPermissionOutcome::new(o.option_id.clone()),
                                    )
                                })
                                .unwrap_or(RequestPermissionOutcome::Cancelled);
                            let _ = responder.respond(RequestPermissionResponse::new(outcome));
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    {
                        let fs_root = fs_root.clone();
                        async move |req: ReadTextFileRequest, responder, _cx| {
                            match read_file_within(&fs_root, &req.path, req.line, req.limit) {
                                Ok(content) => {
                                    let _ = responder.respond(ReadTextFileResponse::new(content));
                                }
                                Err(e) => {
                                    let _ = responder
                                        .respond_with_internal_error(format!("读取失败: {e}"));
                                }
                            }
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(
                    ByteStreams::new(child_stdin, child_stdout),
                    |conn: ConnectionTo<Agent>| async move {
                        let init_req = InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(
                                ClientCapabilities::new()
                                    .fs(FileSystemCapabilities::new().read_text_file(true)),
                            )
                            .client_info(Implementation::new(
                                "RepoMeow",
                                env!("CARGO_PKG_VERSION"),
                            ));
                        let init = match tokio::time::timeout(
                            INIT_TIMEOUT,
                            conn.send_request(init_req).block_task(),
                        )
                        .await
                        {
                            Err(_) => {
                                let _ = hs_tx.send(Err(AppError::coded(
                                    ErrorCode::AgentHandshakeFailed,
                                    "initialize 握手超时(60s;首次经 npx 运行需下载适配器包,网络慢时请重试)",
                                )));
                                return Ok(());
                            }
                            Ok(Err(e)) => {
                                let _ = hs_tx.send(Err(handshake_err(e, &stderr_tail)));
                                return Ok(());
                            }
                            Ok(Ok(resp)) => resp,
                        };
                        let agent_name = init
                            .agent_info
                            .map(|i| i.title.clone().unwrap_or(i.name))
                            .unwrap_or_else(|| display_name.clone());

                        let (session_cwd, apply_config) = match &mode {
                            SessionMode::Test => (std::env::temp_dir(), None),
                            SessionMode::Generate { cwd } => (cwd.clone(), Some((model, thinking))),
                        };
                        let new_session = match conn
                            .send_request(NewSessionRequest::new(session_cwd))
                            .block_task()
                            .await
                        {
                            Ok(s) => s,
                            Err(e) => {
                                let _ = hs_tx.send(Err(handshake_err(e, &stderr_tail)));
                                return Ok(());
                            }
                        };
                        let (config_options, modes) = session_options_snapshot(&new_session);
                        if let Some((model, thinking)) = apply_config {
                            apply_session_config(
                                &conn,
                                &new_session.session_id,
                                &config_options,
                                &modes,
                                model.as_deref(),
                                thinking.as_deref(),
                            )
                            .await;
                        }
                        let _ = hs_tx.send(Ok(AcpHandshake {
                            agent_name,
                            config_options,
                            modes,
                        }));

                        match mode {
                            SessionMode::Test => Ok(()),
                            SessionMode::Generate { .. } => {
                                let session_id = new_session.session_id;
                                loop {
                                    let msg = match job_rx.recv().await {
                                        Some(m) => m,
                                        None => break,
                                    };
                                    match msg {
                                        JobMsg::Prompt { prompt, sender, done } => {
                                            *sink.lock().unwrap() =
                                                Some(PromptSink { sender, text: String::new() });
                                            let mut fut = std::pin::pin!(
                                                conn.send_request(PromptRequest::new(
                                                    session_id.clone(),
                                                    vec![ContentBlock::Text(TextContent::new(prompt))],
                                                ))
                                                .block_task()
                                            );
                                            // 等待响应期间可多次响应取消:发 session/cancel
                                            // 后继续等最终响应(stopReason=cancelled)
                                            let resp = loop {
                                                tokio::select! {
                                                    r = &mut fut => break r,
                                                    _ = cancel.notified() => {
                                                        let _ = conn.send_notification(
                                                            CancelNotification::new(session_id.clone()),
                                                        );
                                                    }
                                                }
                                            };
                                            let text = sink
                                                .lock()
                                                .unwrap()
                                                .take()
                                                .map(|s| s.text)
                                                .unwrap_or_default();
                                            let out = match resp {
                                                Ok(r) => match r.stop_reason {
                                                    StopReason::EndTurn => Ok(AcpPromptResult {
                                                        stop_reason: "end_turn".into(),
                                                        text,
                                                        // PromptResponse.usage 即本次 prompt 消耗
                                                        usage: r.usage.map(Into::into),
                                                    }),
                                                    StopReason::Cancelled => Err(AppError::coded(
                                                        ErrorCode::AgentCanceled,
                                                        "",
                                                    )),
                                                    other => Err(AppError::coded(
                                                        ErrorCode::AgentPromptFailed,
                                                        format!("stop_reason={}", stop_reason_str(other)),
                                                    )),
                                                },
                                                Err(e) => Err(AppError::coded(
                                                    ErrorCode::AgentPromptFailed,
                                                    format!("{e}{}", tail_text(&stderr_tail)),
                                                )),
                                            };
                                            let _ = done.send(out);
                                        }
                                    }
                                }
                                Ok(())
                            }
                        }
                    },
                )
                .await;
            if let Err(e) = result {
                eprintln!("[agent] 连接结束: {e}");
            }
            // 清理:无论成败都移除注册项并确保进程退出
            agent_jobs().lock().unwrap().remove(&run_id);
            agent_pids().lock().unwrap().remove(&pid);
            let _ = child.kill();
        });
    }

    // 等握手结果(成功/失败/连接中断/超时;失败即清理进程并返回)
    let outcome = match tokio::time::timeout(HANDSHAKE_TIMEOUT, hs_rx).await {
        Ok(Ok(Ok(hs))) => AcpStartResult {
            run_id: run_id.clone(),
            agent_name: hs.agent_name,
            config_options: hs.config_options,
            modes: hs.modes,
        },
        Ok(Ok(Err(e))) => return Err(cleanup_start_failure(pid, &run_id, e)),
        Ok(Err(_)) => {
            return Err(cleanup_start_failure(
                pid,
                &run_id,
                AppError::coded(
                    ErrorCode::AgentHandshakeFailed,
                    "连接中断(agent 进程提前退出)",
                ),
            ));
        }
        Err(_) => {
            return Err(cleanup_start_failure(
                pid,
                &run_id,
                AppError::coded(ErrorCode::AgentHandshakeFailed, "握手超时"),
            ));
        }
    };
    agent_jobs().lock().unwrap().insert(
        run_id,
        AgentSession {
            pid,
            job_tx,
            cancel,
        },
    );
    Ok(outcome)
}

/// 启动失败的收尾:记录日志、按 PID 杀进程并移除登记,原样返回错误
fn cleanup_start_failure(pid: u32, run_id: &str, e: AppError) -> AppError {
    eprintln!("[agent] 会话启动失败: {e}");
    agent_pids().lock().unwrap().remove(&pid);
    kill_agent_pid(pid);
    agent_jobs().lock().unwrap().remove(run_id);
    e
}

/// session/new 响应 → IPC 快照:config_options(仅 select 类)与旧式 modes
fn session_options_snapshot(
    resp: &NewSessionResponse,
) -> (Vec<AcpConfigOptionInfo>, Vec<AcpModeInfo>) {
    let config_options = resp
        .config_options
        .iter()
        .flatten()
        .filter(|o| matches!(o.kind, SessionConfigKind::Select(_)))
        .map(config_option_info)
        .collect();
    let modes = resp
        .modes
        .as_ref()
        .map(|m| {
            m.available_modes
                .iter()
                .map(|mode| AcpModeInfo {
                    id: mode.id.to_string(),
                    name: mode.name.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    (config_options, modes)
}

/// 单个会话配置选项 → IPC 信息(分组选项拍平;boolean 类不会被调用进来)
fn config_option_info(opt: &SessionConfigOption) -> AcpConfigOptionInfo {
    let (current, choices) = match &opt.kind {
        SessionConfigKind::Select(sel) => {
            let choices = match &sel.options {
                SessionConfigSelectOptions::Ungrouped(list) => list
                    .iter()
                    .map(|o| AcpConfigChoice {
                        id: o.value.to_string(),
                        name: o.name.clone(),
                    })
                    .collect(),
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|g| {
                        g.options.iter().map(|o| AcpConfigChoice {
                            id: o.value.to_string(),
                            name: o.name.clone(),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            (Some(sel.current_value.to_string()), choices)
        }
        _ => (None, Vec::new()),
    };
    AcpConfigOptionInfo {
        id: opt.id.to_string(),
        name: opt.name.clone(),
        category: category_str(&opt.category),
        current,
        choices,
    }
}

/// 配置选项语义类别 → 稳定字符串(前端按 model/thought_level 挑选项用)
fn category_str(c: &Option<SessionConfigOptionCategory>) -> Option<String> {
    match c {
        Some(SessionConfigOptionCategory::Mode) => Some("mode".into()),
        Some(SessionConfigOptionCategory::Model) => Some("model".into()),
        Some(SessionConfigOptionCategory::ModelConfig) => Some("model_config".into()),
        Some(SessionConfigOptionCategory::ThoughtLevel) => Some("thought_level".into()),
        Some(SessionConfigOptionCategory::Other(s)) => Some(s.clone()),
        // 非穷举枚举:未来新增的具名类别视作未分类
        _ => None,
    }
}

/// 会话创建后应用用户选择的模型/思考强度:优先 config_options 里
/// category=model/thought_level 的项(session/set_config_option),agent 未上报
/// 模型配置项时回退旧式 modes(session/set_mode)。值不在上报列表内或请求失败仅记
/// 日志不阻断会话(生成继续用 agent 默认配置)。
async fn apply_session_config(
    conn: &ConnectionTo<Agent>,
    session_id: &SessionId,
    config_options: &[AcpConfigOptionInfo],
    modes: &[AcpModeInfo],
    model: Option<&str>,
    thinking: Option<&str>,
) {
    if let Some(value) = thinking {
        let opt = config_options
            .iter()
            .find(|o| o.category.as_deref() == Some("thought_level"));
        match opt {
            Some(opt) if opt.choices.iter().any(|c| c.id == value) => {
                set_config_option(conn, session_id, &opt.id, value).await;
            }
            _ => {
                eprintln!("[agent] 未应用思考强度 {value:?}(agent 未上报该选项或不包含此值)");
            }
        }
    }
    if let Some(value) = model {
        let opt = config_options
            .iter()
            .find(|o| o.category.as_deref() == Some("model"));
        if let Some(opt) = opt {
            if opt.choices.iter().any(|c| c.id == value) {
                set_config_option(conn, session_id, &opt.id, value).await;
            } else {
                eprintln!("[agent] 未应用模型选择 {value:?}(不在 agent 上报的模型列表内)");
            }
        } else if modes.iter().any(|m| m.id == value) {
            let req = SetSessionModeRequest::new(session_id.clone(), value.to_string());
            match tokio::time::timeout(CONFIG_TIMEOUT, conn.send_request(req).block_task()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("[agent] set_mode 失败: {e}"),
                Err(_) => eprintln!("[agent] set_mode 超时"),
            }
        } else {
            eprintln!("[agent] 未应用模型选择 {value:?}(不在 agent 上报的模型/mode 列表内)");
        }
    }
}

async fn set_config_option(
    conn: &ConnectionTo<Agent>,
    session_id: &SessionId,
    config_id: &str,
    value: &str,
) {
    let req = SetSessionConfigOptionRequest::new(
        session_id.clone(),
        config_id.to_string(),
        SessionConfigOptionValue::value_id(value.to_string()),
    );
    match tokio::time::timeout(CONFIG_TIMEOUT, conn.send_request(req).block_task()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => eprintln!("[agent] set_config_option({config_id}) 失败: {e}"),
        Err(_) => eprintln!("[agent] set_config_option({config_id}) 超时"),
    }
}

/// 解析启动命令:精选 id 或自定义命令行 → (可执行路径, 参数, 展示名)
fn resolve_spawn(
    agent_id: Option<String>,
    custom_command: Option<String>,
) -> AppResult<(PathBuf, Vec<String>, String)> {
    if let Some(cmdline) = custom_command {
        let tokens = parse_command_line(&cmdline);
        let (program, args) = tokens
            .split_first()
            .ok_or_else(|| AppError::coded(ErrorCode::AgentNotDetected, "自定义命令为空"))?;
        let program_path = resolve_program(program)?;
        return Ok((program_path, args.to_vec(), program.clone()));
    }
    let id =
        agent_id.ok_or_else(|| AppError::coded(ErrorCode::AgentNotDetected, "未指定 agent"))?;
    let def = AGENTS
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| AppError::coded(ErrorCode::AgentNotDetected, format!("未知 agent: {id}")))?;
    match &def.kind {
        AgentKind::Npx { pkg, args } => {
            let npx = which::which("npx").map_err(|_| {
                AppError::coded(
                    ErrorCode::AgentNotDetected,
                    format!("{} 需要 Node.js(npx)", def.name),
                )
            })?;
            let mut full = vec!["-y".to_string(), (*pkg).to_string()];
            full.extend(args.iter().map(|s| s.to_string()));
            Ok((npx, full, def.name.to_string()))
        }
        AgentKind::Binary { cmd, args } => {
            let path = resolve_program(cmd).map_err(|_| {
                AppError::coded(ErrorCode::AgentNotDetected, format!("未检测到 {cmd} 命令"))
            })?;
            Ok((
                path,
                args.iter().map(|s| s.to_string()).collect(),
                def.name.to_string(),
            ))
        }
    }
}

/// 程序名解析:含路径分隔符直接用,否则从 PATH 解析(Windows 由 which 处理 PATHEXT/.cmd)
fn resolve_program(program: &str) -> AppResult<PathBuf> {
    if program.contains('/') || program.contains('\\') || Path::new(program).is_absolute() {
        Ok(PathBuf::from(program))
    } else {
        which::which(program).map_err(|_| {
            AppError::coded(
                ErrorCode::AgentNotDetected,
                format!("未找到命令: {program}"),
            )
        })
    }
}

/// 简单命令行分词:空白分隔,双引号内保留空格(不含转义序列)
fn parse_command_line(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn stop_reason_str(r: StopReason) -> &'static str {
    match r {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
}

/// stderr 尾部文本(取后 2KB,供错误信息附加诊断)
fn tail_text(tail: &Arc<Mutex<Vec<u8>>>) -> String {
    let t = tail.lock().unwrap();
    let start = t.len().saturating_sub(2048);
    String::from_utf8_lossy(&t[start..]).trim().to_string()
}

/// 按 PID 强杀进程树:Windows taskkill /T /F(覆盖 npx→node 包装链);
/// unix 杀负 PID 进程组(spawn 时已 process_group(0))
fn kill_agent_pid(pid: u32) {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let _ = cmd.output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .output();
    }
}

/// session/update → 前端事件:正文块累积推送;工具调用转活动行;其余忽略
fn route_session_update(sink: &SharedSink, update: SessionUpdate) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(t) = chunk.content {
                push_chunk(sink, &t.text);
            }
        }
        SessionUpdate::ToolCall(call) => {
            let title = if call.title.is_empty() {
                format!("{:?}", call.kind)
            } else {
                call.title
            };
            push_activity(sink, format!("工具: {title}"));
        }
        _ => {}
    }
}

fn push_chunk(sink: &SharedSink, delta: &str) {
    let (sender, text) = {
        let mut guard = sink.lock().unwrap();
        match guard.as_mut() {
            Some(s) => {
                s.text.push_str(delta);
                (s.sender.clone(), s.text.clone())
            }
            None => return,
        }
    };
    sender.send(AcpEvent::Chunk { text });
}

fn push_activity(sink: &SharedSink, text: String) {
    if let Some(s) = sink.lock().unwrap().as_ref() {
        s.sender.send(AcpEvent::Activity { text });
    }
}

/// fs/read_text_file 回调实现:路径限制在 root 内(canonicalize 防越界),
/// 支持 1-based 起始行与行数上限,总量限 READ_FILE_MAX
fn read_file_within(
    root: &Path,
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> std::io::Result<String> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canon = full.canonicalize()?;
    if !canon.starts_with(&root_canon) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("路径越界: {}", canon.display()),
        ));
    }
    let bytes = std::fs::read(&canon)?;
    if bytes.len() > READ_FILE_MAX {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            format!("超过 {} 字节上限", READ_FILE_MAX),
        ));
    }
    let content = String::from_utf8_lossy(&bytes).to_string();
    match (line, limit) {
        (None, None) => Ok(content),
        _ => {
            let start = line.unwrap_or(1).saturating_sub(1) as usize;
            let take = limit.unwrap_or(u32::MAX) as usize;
            let sliced: Vec<&str> = content.lines().skip(start).take(take).collect();
            Ok(sliced.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_option_info_maps_category_and_flattens_groups() {
        use agent_client_protocol::schema::v1::{
            SessionConfigSelectGroup, SessionConfigSelectOption,
        };

        let opt = SessionConfigOption::select(
            "model",
            "Model",
            "glm-4.6",
            vec![
                SessionConfigSelectOption::new("glm-4.6", "GLM-4.6"),
                SessionConfigSelectOption::new("glm-4.5", "GLM-4.5"),
            ],
        )
        .category(SessionConfigOptionCategory::Model);
        let info = config_option_info(&opt);
        assert_eq!(info.id, "model");
        assert_eq!(info.category.as_deref(), Some("model"));
        assert_eq!(info.current.as_deref(), Some("glm-4.6"));
        assert_eq!(info.choices.len(), 2);
        assert_eq!(info.choices[1].id, "glm-4.5");
        assert_eq!(info.choices[1].name, "GLM-4.5");

        // 分组选项拍平 + thought_level 类别
        let grouped = SessionConfigOption::select(
            "effort",
            "Effort",
            "low",
            vec![SessionConfigSelectGroup::new(
                "g1",
                "常用",
                vec![SessionConfigSelectOption::new("low", "Low")],
            )],
        )
        .category(SessionConfigOptionCategory::ThoughtLevel);
        let info = config_option_info(&grouped);
        assert_eq!(info.category.as_deref(), Some("thought_level"));
        assert_eq!(
            info.choices,
            vec![AcpConfigChoice {
                id: "low".into(),
                name: "Low".into()
            }]
        );

        assert_eq!(
            category_str(&Some(SessionConfigOptionCategory::Other("custom".into()))).as_deref(),
            Some("custom"),
        );
        assert_eq!(category_str(&None), None);
    }

    #[test]
    fn parse_command_line_splits_and_quotes() {
        assert_eq!(
            parse_command_line("npx -y pkg --acp"),
            ["npx", "-y", "pkg", "--acp"]
        );
        assert_eq!(
            parse_command_line(r#""C:\Program Files\agent.exe" --acp "a b""#),
            ["C:\\Program Files\\agent.exe", "--acp", "a b"]
        );
        assert!(parse_command_line("   ").is_empty());
    }

    #[test]
    fn read_file_within_respects_root_and_range() {
        let dir = std::env::temp_dir().join(format!("repomeow-agent-test-{}", now_ts_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "l1\nl2\nl3\n").unwrap();

        // 相对路径 + 行区间
        assert_eq!(
            read_file_within(&dir, Path::new("a.txt"), Some(2), Some(1)).unwrap(),
            "l2"
        );
        // 绝对路径
        assert_eq!(
            read_file_within(&dir, &file, None, None).unwrap(),
            "l1\nl2\nl3\n"
        );
        // 越界(指向 root 之外)拒绝
        assert!(read_file_within(&dir, Path::new("../escape.txt"), None, None).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
