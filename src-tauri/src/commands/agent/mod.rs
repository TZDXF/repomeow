//! wiki agent 后端:经 ACP(Agent Client Protocol,JSON-RPC/NDJSON over stdio)
//! 调用用户本地已认证的 coding agent CLI。
//!
//! 模块职责:
//! - `registry`:精选 agent 清单、安装探测与启动命令解析
//! - `process`:子进程 spawn、stderr 采集与进程树清理
//! - `session`:ACP 连接、握手、prompt 驱动与会话生命周期
//! - `config`:协议原生模型/思考强度配置映射
//! - `callbacks`:流式事件、工具摘要、权限决策与受限文件读取

mod callbacks;
mod config;
pub(crate) mod conflict;
mod process;
mod registry;
mod session;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_client_protocol::schema::v1::Usage;
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot, Notify};

pub use config::{AcpConfigOptionInfo, AcpModeInfo};
pub use registry::AgentInfo;

use process::kill_agent_pid;
use session::run_session;

use crate::error::{AppError, AppResult, ErrorCode};

const CANCEL_GRACE: Duration = Duration::from_secs(5);

#[tauri::command]
pub fn agent_list() -> AppResult<Vec<AgentInfo>> {
    Ok(registry::list_agents())
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AcpEvent {
    Chunk { text: String },
    Activity { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigChoice {
    id: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpStartResult {
    pub(crate) run_id: String,
    pub(crate) agent_name: String,
    config_options: Vec<AcpConfigOptionInfo>,
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
    pub(crate) stop_reason: String,
    pub(crate) text: String,
    pub(crate) usage: Option<AcpTokenUsage>,
}

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
    fn from(usage: Usage) -> Self {
        Self {
            total_tokens: usage.total_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            thought_tokens: usage.thought_tokens,
            cached_read_tokens: usage.cached_read_tokens,
            cached_write_tokens: usage.cached_write_tokens,
        }
    }
}

enum JobMsg {
    Prompt {
        prompt: String,
        sender: AcpEventSender,
        done: oneshot::Sender<Result<AcpPromptResult, AppError>>,
    },
}

enum SessionMode {
    Generate { cwd: PathBuf, access: AgentAccess },
    Test,
}

#[derive(Clone, Copy)]
enum AgentAccess {
    ReadOnly,
    WorkspaceWrite,
}

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

static AGENT_PIDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

fn agent_pids() -> &'static Mutex<HashSet<u32>> {
    AGENT_PIDS.get_or_init(|| Mutex::new(HashSet::new()))
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
            access: AgentAccess::ReadOnly,
        },
        model,
        thinking,
        cwd,
    )
    .await
}

/// 显式用户操作触发的代码冲突解决会话：允许 agent 在工作区内写文件并执行工具。
pub(crate) async fn acp_start_writable(agent_id: String, cwd: String) -> AppResult<AcpStartResult> {
    run_session(
        Some(agent_id),
        None,
        SessionMode::Generate {
            cwd: PathBuf::from(&cwd),
            access: AgentAccess::WorkspaceWrite,
        },
        None,
        None,
        cwd,
    )
    .await
}

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
        Ok(result) => result,
        Err(_) => Err(AppError::coded(
            ErrorCode::AgentPromptFailed,
            "会话在生成中中断(agent 进程退出)",
        )),
    }
}

#[tauri::command]
pub fn acp_cancel(run_id: String) -> AppResult<()> {
    let session = agent_jobs()
        .lock()
        .unwrap()
        .remove(&run_id)
        .ok_or_else(|| AppError::coded(ErrorCode::AgentPromptFailed, "会话不存在或已结束"))?;
    session.cancel.notify_one();
    let pid = session.pid;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CANCEL_GRACE).await;
        if agent_pids().lock().unwrap().contains(&pid) {
            kill_agent_pid(pid);
        }
    });
    Ok(())
}

pub fn cleanup_on_exit() {
    let pids: Vec<u32> = agent_pids().lock().unwrap().drain().collect();
    for pid in pids {
        kill_agent_pid(pid);
    }
}

#[cfg(test)]
mod tests;
