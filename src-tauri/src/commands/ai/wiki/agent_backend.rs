use super::*;

/// agent 后端的会话参数。每个页面(及每次重试)独立建会话:长会话上下文累积是
/// max_tokens/max_turn_requests 类中断的主因,独立会话让每个 prompt 都从干净上下文开始;
/// 也正因为会话互不共享状态,页面可以按 concurrency 并行生成。
#[derive(Clone)]
pub(super) struct AgentSessionParams {
    pub(super) agent_id: Option<String>,
    pub(super) custom_command: Option<String>,
    pub(super) cwd: String,
    pub(super) model: Option<String>,
    pub(super) thinking: Option<String>,
    /// 页面并发数(1-8,默认 2)
    pub(super) concurrency: usize,
}

impl AgentSessionParams {
    pub(super) fn from_backend(backend: &WikiGenerationBackend, cwd: &str) -> Option<Self> {
        let WikiGenerationBackend::Agent {
            agent_id,
            custom_command,
            model,
            thinking,
            concurrency,
        } = backend
        else {
            return None;
        };
        Some(Self {
            agent_id: agent_id.clone(),
            custom_command: custom_command.clone(),
            cwd: cwd.to_string(),
            model: model.clone(),
            thinking: thinking.clone(),
            concurrency: concurrency.filter(|c| *c > 0).unwrap_or(2).clamp(1, 8),
        })
    }

    /// 用量/元信息里的模型标识:agent 名称 · 所选模型(未选模型则只有名称)
    pub(super) fn usage_model(&self, agent_name: &str) -> String {
        self.model
            .as_ref()
            .map(|model| format!("{agent_name} · {model}"))
            .unwrap_or_else(|| agent_name.to_string())
    }
}

/// 当前进行中的 agent 会话集合;整本生成期间随大纲/页面推进轮换(并发页面
/// 会同时存在多个会话),取消时终止集合内全部会话(杀进程树)
#[derive(Clone, Default)]
pub(super) struct AgentSessionSlot(Arc<Mutex<HashSet<String>>>);

impl AgentSessionSlot {
    pub(super) fn track(&self, run_id: &str) {
        self.0.lock().unwrap().insert(run_id.to_string());
    }

    pub(super) fn untrack(&self, run_id: &str) {
        self.0.lock().unwrap().remove(run_id);
    }

    pub(super) fn cancel_all(&self) {
        let ids: Vec<String> = self.0.lock().unwrap().drain().collect();
        for id in ids {
            let _ = agent::acp_cancel(id);
        }
    }
}

/// 取消监视:运行取消时终止槽位里的全部 agent 会话
pub(super) fn watch_agent_cancel(
    token: CancellationToken,
    slot: AgentSessionSlot,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        token.cancelled().await;
        slot.cancel_all();
    })
}

/// 建立一次 agent 会话并登记到槽位
pub(super) async fn open_agent_session(
    params: &AgentSessionParams,
    slot: &AgentSessionSlot,
) -> AppResult<agent::AcpStartResult> {
    let started = agent::acp_start(
        params.agent_id.clone(),
        params.custom_command.clone(),
        params.cwd.clone(),
        params.model.clone(),
        params.thinking.clone(),
    )
    .await?;
    slot.track(&started.run_id);
    Ok(started)
}

/// 结束会话:从槽位移除 + 取消(进程由 acp_cancel 的宽限杀与驱动任务收尾兜底)
pub(super) fn close_agent_session(slot: &AgentSessionSlot, run_id: &str) {
    slot.untrack(run_id);
    let _ = agent::acp_cancel(run_id.to_string());
}

/// 单页生成的运行统计(进度面板展示耗时;usage_model 回填 meta)
pub(super) struct AgentPageStats {
    pub(super) duration_ms: u64,
    pub(super) usage_model: String,
}

pub(super) async fn generate_agent_outline_pages(
    db: &Db,
    run_id: &str,
    usage_model: &str,
    context: &wiki::WikiContext,
    project_name: &str,
    language: &str,
    on_activity: Arc<dyn Fn(String) + Send + Sync>,
    on_retry: Arc<dyn Fn(WikiRetryNotice) + Send + Sync>,
) -> AppResult<Vec<wiki::WikiOutlinePage>> {
    let original_prompt = agent_wiki_outline_prompt(context, project_name, language);
    let mut prompt = original_prompt.clone();
    let valid_files: HashSet<String> = context.paths.iter().cloned().collect();
    let mut last_error = "wiki outline JSON was not generated".to_string();
    const MAX_ATTEMPTS: usize = 3;
    for _ in 1..=MAX_ATTEMPTS {
        let started = Instant::now();
        let activity = on_activity.clone();
        let result = agent::acp_prompt_with(
            run_id.to_string(),
            prompt.clone(),
            agent::AcpEventSender::new(move |event| match event {
                agent::AcpEvent::Chunk { .. } => {}
                agent::AcpEvent::Activity { text } => activity(text),
            }),
        )
        .await?;
        record_acp_usage(
            db,
            usage_model,
            &prompt,
            &result,
            started.elapsed().as_millis() as i64,
        );
        // 非正常停止(max_tokens/max_turn_requests 等):部分 JSON 无纠错价值,
        // 用原始 prompt 重新生成;refusal 重试无意义,快速失败
        if result.stop_reason != "end_turn" {
            if result.stop_reason == "refusal" {
                return Err(AppError::coded(
                    ErrorCode::AgentPromptFailed,
                    "agent 拒绝生成大纲(stop_reason=refusal)",
                ));
            }
            last_error = format!("agent 中途停止(stop_reason={})", result.stop_reason);
            prompt = original_prompt.clone();
            continue;
        }
        let (cleaned, retry) = sanitize_agent_retry_notices(&result.text);
        if let Some(notice) = retry {
            on_retry(notice);
        }
        let outline = sdk::strip_thinking(&cleaned);
        match crate::ai::wiki_outline::parse_outline(&outline, &valid_files) {
            Ok(pages) => return Ok(pages),
            Err(error) => {
                last_error = error;
                prompt = outline_retry_prompt(&original_prompt, &last_error);
            }
        }
    }
    Err(AppError::coded(
        ErrorCode::AiResponseParseFailed,
        last_error,
    ))
}

pub(super) async fn generate_agent_page_to_disk(
    app: &AppHandle,
    db: &Db,
    params: &AgentSessionParams,
    slot: &AgentSessionSlot,
    page: &wiki::WikiOutlinePage,
    language: &str,
    changed_files: &[String],
    cancel: &CancellationToken,
    on_progress: Arc<dyn Fn(String) + Send + Sync>,
    on_activity: Arc<dyn Fn(String) + Send + Sync>,
    on_retry: Arc<dyn Fn(WikiRetryNotice) + Send + Sync>,
) -> AppResult<AgentPageStats> {
    // 混合模式:相关文件全文(与内置后端同预算)直接进 prompt,agent 不再逐文件工具调用
    let files = wiki::read_wiki_files_in(&params.cwd, &page.relevant_files)?;
    let prompt = agent_wiki_page_prompt(page, &files, changed_files, language);
    let page_started = Instant::now();
    let mut last_error = None;
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        if cancel.is_cancelled() {
            return Err(AppError::coded(ErrorCode::AgentCanceled, ""));
        }
        // 每次尝试独立会话:重试不再背负上一轮的上下文(可能正是 max_tokens 的成因)
        let started = match open_agent_session(params, slot).await {
            Ok(started) => started,
            Err(error) => {
                // agent 未安装/未登录没有重试意义
                let fatal = error.code() == "agent_not_detected";
                last_error = Some(error);
                if fatal {
                    break;
                }
                if attempt < MAX_ATTEMPTS {
                    let notice = retry_notice(last_error.as_ref().unwrap(), attempt, MAX_ATTEMPTS);
                    on_retry(notice.clone());
                    wait_for_wiki_retry(cancel, &notice).await?;
                }
                continue;
            }
        };
        on_progress(String::new());
        let progress = on_progress.clone();
        let activity = on_activity.clone();
        let retry = on_retry.clone();
        let seen_retry = Arc::new(Mutex::new(None::<(usize, usize)>));
        let sender = agent::AcpEventSender::new(move |event| match event {
            agent::AcpEvent::Chunk { text } => {
                let (cleaned, notice) = sanitize_agent_retry_notices(&text);
                progress(sdk::strip_thinking(&cleaned));
                if let Some(notice) = notice {
                    let marker = (notice.attempt, notice.max_attempts);
                    let mut seen = seen_retry.lock().unwrap();
                    if seen.as_ref() != Some(&marker) {
                        *seen = Some(marker);
                        retry(notice);
                    }
                }
            }
            agent::AcpEvent::Activity { text } => activity(text),
        });
        let attempt_started = Instant::now();
        let outcome = agent::acp_prompt_with(started.run_id.clone(), prompt.clone(), sender).await;
        close_agent_session(slot, &started.run_id);
        let usage_model = params.usage_model(&started.agent_name);
        match outcome {
            Ok(result) => {
                record_acp_usage(
                    db,
                    &usage_model,
                    &prompt,
                    &result,
                    attempt_started.elapsed().as_millis() as i64,
                );
                match result.stop_reason.as_str() {
                    "end_turn" => {
                        let (cleaned, _) = sanitize_agent_retry_notices(&result.text);
                        let page_content = sdk::strip_thinking(&cleaned);
                        if page_content.is_empty() {
                            last_error = Some(AppError::coded(
                                ErrorCode::AgentPromptFailed,
                                "wiki page response is empty after removing thinking content",
                            ));
                        } else {
                            wiki::save_wiki_page_internal(
                                app,
                                &params.cwd,
                                &page.file,
                                &page_content,
                            )?;
                            return Ok(AgentPageStats {
                                duration_ms: page_started.elapsed().as_millis() as u64,
                                usage_model,
                            });
                        }
                    }
                    // 模型拒绝:重试同一 prompt 无意义,快速失败
                    "refusal" => {
                        return Err(AppError::coded(
                            ErrorCode::AgentPromptFailed,
                            "agent 拒绝生成该页(stop_reason=refusal)",
                        ));
                    }
                    // max_tokens/max_turn_requests/unknown:下次尝试换新会话重试
                    other => {
                        last_error = Some(AppError::coded(
                            ErrorCode::AgentPromptFailed,
                            format!("agent 中途停止(stop_reason={other})"),
                        ));
                    }
                }
            }
            Err(error) => {
                if cancel.is_cancelled() || error.code() == "agent_canceled" {
                    return Err(error);
                }
                last_error = Some(error);
            }
        }
        if attempt < MAX_ATTEMPTS {
            let notice = retry_notice(last_error.as_ref().unwrap(), attempt, MAX_ATTEMPTS);
            on_retry(notice.clone());
            wait_for_wiki_retry(cancel, &notice).await?;
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::coded(ErrorCode::AgentPromptFailed, "wiki page generation failed")
    }))
}
