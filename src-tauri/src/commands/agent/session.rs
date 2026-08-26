use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, FileSystemCapabilities, Implementation,
    InitializeRequest, NewSessionRequest, PromptRequest, ReadTextFileRequest, ReadTextFileResponse,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification, StopReason,
    TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use tokio::sync::{mpsc, oneshot, Notify};

use super::callbacks::{
    decide_permission, push_activity, read_file_within, read_tool_activity_text,
    route_session_update, PromptSink, SharedSink,
};
use super::config::{apply_session_config, session_options_snapshot};
use super::process::{capture_stderr, kill_agent_pid, spawn_agent, tail_text};
use super::registry::resolve_spawn;
use super::{
    agent_jobs, agent_pids, AcpHandshake, AcpPromptResult, AcpStartResult, AgentSession, JobMsg,
    SessionMode,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::time_util::now_ts_nanos;

const INIT_TIMEOUT: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(75);
const CANCEL_GRACE: Duration = Duration::from_secs(5);
const PROMPT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// spawn + 握手 + 注册;握手失败时清理并返回错误。
pub(super) async fn run_session(
    agent_id: Option<String>,
    custom_command: Option<String>,
    mode: SessionMode,
    model: Option<String>,
    thinking: Option<String>,
    fs_root: String,
) -> AppResult<AcpStartResult> {
    let (program, args, display_name) = resolve_spawn(agent_id, custom_command)?;
    let spawned = spawn_agent(&program, &args)?;
    let pid = spawned.pid;
    let mut child = spawned.child;
    let child_stdin = spawned.stdin;
    let child_stdout = spawned.stdout;
    let stderr_tail = capture_stderr(spawned.stderr);

    let run_id = format!("acp-{}", now_ts_nanos());
    let (job_tx, mut job_rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(Notify::new());
    let sink: SharedSink = Arc::new(Mutex::new(None));
    let (hs_tx, hs_rx) = oneshot::channel::<Result<AcpHandshake, AppError>>();

    agent_pids().lock().unwrap().insert(pid);
    let fs_root = PathBuf::from(&fs_root);

    {
        let run_id = run_id.clone();
        let cancel = cancel.clone();
        let sink = sink.clone();
        let stderr_tail = stderr_tail.clone();
        tauri::async_runtime::spawn(async move {
            let handshake_err = |error: agent_client_protocol::Error,
                                 tail: &Arc<Mutex<Vec<u8>>>| {
                AppError::coded(
                    ErrorCode::AgentHandshakeFailed,
                    format!("{error}{}", tail_text(tail)),
                )
            };
            let result = Client
                .builder()
                .on_receive_notification(
                    {
                        let sink = sink.clone();
                        async move |notification: SessionNotification, _cx| {
                            route_session_update(&sink, notification.update);
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    {
                        let sink = sink.clone();
                        async move |req: RequestPermissionRequest, responder, _cx| {
                            let title = req.tool_call.fields.title.clone().unwrap_or_default();
                            let (allowed, outcome) = decide_permission(&req);
                            push_activity(
                                &sink,
                                if allowed {
                                    format!("已允许: {title}")
                                } else {
                                    format!("已拒绝(生成 wiki 只读): {title}")
                                },
                            );
                            let _ = responder.respond(RequestPermissionResponse::new(outcome));
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    {
                        let fs_root = fs_root.clone();
                        let sink = sink.clone();
                        async move |req: ReadTextFileRequest, responder, _cx| {
                            push_activity(
                                &sink,
                                read_tool_activity_text(&req.path, req.line, req.limit),
                            );
                            match read_file_within(&fs_root, &req.path, req.line, req.limit) {
                                Ok(content) => {
                                    let _ = responder.respond(ReadTextFileResponse::new(content));
                                }
                                Err(error) => {
                                    let _ = responder
                                        .respond_with_internal_error(format!("读取失败: {error}"));
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
                            Ok(Err(error)) => {
                                let _ = hs_tx.send(Err(handshake_err(error, &stderr_tail)));
                                return Ok(());
                            }
                            Ok(Ok(response)) => response,
                        };
                        let agent_name = init
                            .agent_info
                            .map(|info| info.title.clone().unwrap_or(info.name))
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
                            Ok(session) => session,
                            Err(error) => {
                                let _ = hs_tx.send(Err(handshake_err(error, &stderr_tail)));
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
                                run_prompt_loop(
                                    &conn,
                                    new_session.session_id,
                                    &mut job_rx,
                                    &cancel,
                                    &sink,
                                    &stderr_tail,
                                )
                                .await;
                                Ok(())
                            }
                        }
                    },
                )
                .await;
            if let Err(error) = result {
                eprintln!("[agent] 连接结束: {error}");
            }
            agent_jobs().lock().unwrap().remove(&run_id);
            agent_pids().lock().unwrap().remove(&pid);
            let _ = child.kill();
        });
    }

    let outcome = match tokio::time::timeout(HANDSHAKE_TIMEOUT, hs_rx).await {
        Ok(Ok(Ok(handshake))) => AcpStartResult {
            run_id: run_id.clone(),
            agent_name: handshake.agent_name,
            config_options: handshake.config_options,
            modes: handshake.modes,
        },
        Ok(Ok(Err(error))) => return Err(cleanup_start_failure(pid, &run_id, error)),
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

async fn run_prompt_loop(
    conn: &ConnectionTo<Agent>,
    session_id: agent_client_protocol::schema::v1::SessionId,
    job_rx: &mut mpsc::UnboundedReceiver<JobMsg>,
    cancel: &Notify,
    sink: &SharedSink,
    stderr_tail: &Arc<Mutex<Vec<u8>>>,
) {
    while let Some(message) = job_rx.recv().await {
        let JobMsg::Prompt {
            prompt,
            sender,
            done,
        } = message;
        *sink.lock().unwrap() = Some(PromptSink::new(sender));
        let mut request = std::pin::pin!(conn
            .send_request(PromptRequest::new(
                session_id.clone(),
                vec![ContentBlock::Text(TextContent::new(prompt))],
            ))
            .block_task());
        let mut timeout = std::pin::pin!(tokio::time::sleep(PROMPT_TIMEOUT));
        enum Wait<T> {
            Done(T),
            TimedOut,
        }
        let response = loop {
            tokio::select! {
                result = &mut request => break Wait::Done(result),
                _ = cancel.notified() => {
                    let _ = conn.send_notification(CancelNotification::new(session_id.clone()));
                }
                _ = &mut timeout => {
                    let _ = conn.send_notification(CancelNotification::new(session_id.clone()));
                    let _ = tokio::time::timeout(CANCEL_GRACE, &mut request).await;
                    break Wait::TimedOut;
                }
            }
        };
        let text = sink
            .lock()
            .unwrap()
            .take()
            .map(|prompt| prompt.text)
            .unwrap_or_default();
        if matches!(response, Wait::TimedOut) {
            let _ = done.send(Err(AppError::coded(
                ErrorCode::AgentPromptFailed,
                format!(
                    "prompt 超过 {}s 未结束(prompt_timeout)",
                    PROMPT_TIMEOUT.as_secs()
                ),
            )));
            break;
        }
        let Wait::Done(response) = response else {
            unreachable!()
        };
        let output = match response {
            Ok(response) => match response.stop_reason {
                StopReason::Cancelled => Err(AppError::coded(ErrorCode::AgentCanceled, "")),
                reason => Ok(AcpPromptResult {
                    stop_reason: stop_reason_str(reason).into(),
                    text,
                    usage: response.usage.map(Into::into),
                }),
            },
            Err(error) => Err(AppError::ai_provider_error(
                ErrorCode::AgentPromptFailed,
                format!("{error}{}", tail_text(stderr_tail)),
            )),
        };
        let _ = done.send(output);
    }
}

fn cleanup_start_failure(pid: u32, run_id: &str, error: AppError) -> AppError {
    eprintln!("[agent] 会话启动失败: {error}");
    agent_pids().lock().unwrap().remove(&pid);
    kill_agent_pid(pid);
    agent_jobs().lock().unwrap().remove(run_id);
    error
}

fn stop_reason_str(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        _ => "unknown",
    }
}
