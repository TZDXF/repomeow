use super::*;

/// 整本 Wiki 的收集、大纲、并发/顺序页生成、重试与最终落盘全部在后端执行。
#[tauri::command]
pub async fn ai_generate_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateWikiRequest,
    on_event: Channel<WikiGenerationEvent>,
) -> AppResult<()> {
    let run = RegisteredRun::new(request.run_id);
    let backend = wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "collecting".into(),
        },
    );
    let context = match wiki::collect_wiki_context(request.project_path.clone()) {
        Ok(context) => context,
        Err(error) => return fail_wiki_generation(&on_event, &run.token, error),
    };
    for paths in context.paths.chunks(24) {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::ActivityBatch {
                activity_type: "scan".into(),
                items: paths.to_vec(),
            },
        );
    }
    let mut read_files = context
        .manifests
        .iter()
        .map(|manifest| manifest.path.clone())
        .collect::<Vec<_>>();
    if context.readme.is_some() {
        read_files.insert(0, "README".into());
    }
    if !read_files.is_empty() {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::ActivityBatch {
                activity_type: "read".into(),
                items: read_files,
            },
        );
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Context {
            file_count: context.file_count,
            tree_truncated: context.tree_truncated,
            has_readme: context.readme.is_some(),
            manifest_count: context.manifests.len(),
        },
    );
    let backend_id;
    let meta_model;
    let mut agent_params = None;
    let mut agent_slot = None;
    let mut agent_cancel_watch = None;

    let pages_result = match &backend {
        WikiGenerationBackend::Builtin => {
            backend_id = "builtin".to_string();
            meta_model = sdk::load_config(&app).ai_model;
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: "outlining".into(),
                },
            );
            generate_builtin_outline_pages(
                &app,
                &db,
                &context,
                &request.project_name,
                &request.language,
                &run.token,
                {
                    let channel = on_event.clone();
                    move |notice| {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Retry {
                                page_id: None,
                                attempt: notice.attempt,
                                max_attempts: notice.max_attempts,
                                delay_seconds: notice.delay_seconds,
                                reason: notice.reason,
                            },
                        );
                    }
                },
            )
            .await
        }
        WikiGenerationBackend::Agent { .. } => {
            let params = AgentSessionParams::from_backend(&backend, &request.project_path)
                .expect("agent backend");
            let slot = AgentSessionSlot::default();
            agent_cancel_watch = Some(watch_agent_cancel(run.token.clone(), slot.clone()));
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: "outlining".into(),
                },
            );
            // 大纲用一个独立会话(纠错重试复用同一会话以保留上下文);
            // 页面生成在下方逐页另起会话
            let started = match open_agent_session(&params, &slot).await {
                Ok(started) => started,
                Err(error) => {
                    if let Some(watch) = agent_cancel_watch.take() {
                        watch.abort();
                    }
                    return fail_wiki_generation(&on_event, &run.token, error);
                }
            };
            backend_id = format!("acp:{}", params.agent_id.as_deref().unwrap_or("custom"));
            meta_model = params.usage_model(&started.agent_name);
            let result = generate_agent_outline_pages(
                &db,
                &started.run_id,
                &meta_model,
                &context,
                &request.project_name,
                &request.language,
                {
                    let channel = on_event.clone();
                    Arc::new(move |text| {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::ActivityBatch {
                                activity_type: "tool".into(),
                                items: vec![text],
                            },
                        );
                    })
                },
                {
                    let channel = on_event.clone();
                    Arc::new(move |notice: WikiRetryNotice| {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Retry {
                                page_id: None,
                                attempt: notice.attempt,
                                max_attempts: notice.max_attempts,
                                delay_seconds: notice.delay_seconds,
                                reason: notice.reason,
                            },
                        );
                    })
                },
            )
            .await;
            close_agent_session(&slot, &started.run_id);
            agent_params = Some(params);
            agent_slot = Some(slot);
            result
        }
    };

    let pages = match pages_result {
        Ok(pages) => pages,
        Err(error) => {
            if let Some(watch) = agent_cancel_watch.take() {
                watch.abort();
            }
            if let Some(slot) = &agent_slot {
                slot.cancel_all();
            }
            let phase = if run.token.is_cancelled() {
                "cancelled"
            } else {
                "failed"
            };
            send_wiki_event(
                &on_event,
                WikiGenerationEvent::Phase {
                    phase: phase.into(),
                },
            );
            return Err(error);
        }
    };
    if run.token.is_cancelled() {
        if let Some(watch) = agent_cancel_watch.take() {
            watch.abort();
        }
        if let Some(slot) = &agent_slot {
            slot.cancel_all();
        }
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(());
    }

    if let Err(error) = wiki::begin_wiki(app.clone(), request.project_path.clone()) {
        if let Some(watch) = agent_cancel_watch.take() {
            watch.abort();
        }
        if let Some(slot) = &agent_slot {
            slot.cancel_all();
        }
        return fail_wiki_generation(&on_event, &run.token, error);
    }
    for page in &pages {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Page {
                page: page.clone(),
                status: "pending".into(),
                error: None,
                duration_ms: None,
            },
        );
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "generating".into(),
        },
    );

    let page_errors = Arc::new(Mutex::new(Vec::<AppError>::new()));
    match &backend {
        WikiGenerationBackend::Builtin => {
            stream::iter(pages.clone())
                .for_each_concurrent(request.concurrency.clamp(1, 8), |page| {
                    let app = app.clone();
                    let db = &db;
                    let project_path = request.project_path.clone();
                    let language = request.language.clone();
                    let token = run.token.clone();
                    let channel = on_event.clone();
                    let page_errors = page_errors.clone();
                    async move {
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page: page.clone(),
                                status: "running".into(),
                                error: None,
                                duration_ms: None,
                            },
                        );
                        let page_started = Instant::now();
                        let progress_channel = channel.clone();
                        let retry_channel = channel.clone();
                        let progress_page_id = page.id.clone();
                        let retry_page_id = page.id.clone();
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::ActivityBatch {
                                activity_type: "read".into(),
                                items: page.relevant_files.clone(),
                            },
                        );
                        let result = generate_builtin_page_to_disk(
                            &app,
                            db,
                            &project_path,
                            &page,
                            &language,
                            &token,
                            move |content| {
                                send_wiki_event(
                                    &progress_channel,
                                    WikiGenerationEvent::Progress {
                                        page_id: progress_page_id.clone(),
                                        content: content.to_string(),
                                    },
                                );
                            },
                            move |notice| {
                                send_wiki_event(
                                    &retry_channel,
                                    WikiGenerationEvent::Retry {
                                        page_id: Some(retry_page_id.clone()),
                                        attempt: notice.attempt,
                                        max_attempts: notice.max_attempts,
                                        delay_seconds: notice.delay_seconds,
                                        reason: notice.reason,
                                    },
                                );
                            },
                        )
                        .await;
                        let (status, error) = if token.is_cancelled() {
                            ("cancelled", None)
                        } else {
                            match result {
                                Ok(()) => ("done", None),
                                Err(error) => {
                                    let message = error.to_string();
                                    page_errors.lock().unwrap().push(error);
                                    ("failed", Some(message))
                                }
                            }
                        };
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page,
                                status: status.into(),
                                error,
                                duration_ms: Some(page_started.elapsed().as_millis() as u64),
                            },
                        );
                    }
                })
                .await;
        }
        WikiGenerationBackend::Agent { .. } => {
            let params = agent_params.as_ref().expect("agent params").clone();
            let slot = agent_slot.as_ref().expect("agent slot").clone();
            // 每页独立会话,互不共享上下文,可以按配置并发(默认 2,上限 8)
            stream::iter(pages.clone())
                .for_each_concurrent(params.concurrency, |page| {
                    let app = app.clone();
                    let db = &db;
                    let params = params.clone();
                    let slot = slot.clone();
                    let token = run.token.clone();
                    let channel = on_event.clone();
                    let language = request.language.clone();
                    let page_errors = page_errors.clone();
                    async move {
                        if token.is_cancelled() {
                            send_wiki_event(
                                &channel,
                                WikiGenerationEvent::Page {
                                    page,
                                    status: "cancelled".into(),
                                    error: None,
                                    duration_ms: None,
                                },
                            );
                            return;
                        }
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page: page.clone(),
                                status: "running".into(),
                                error: None,
                                duration_ms: None,
                            },
                        );
                        let progress_channel = channel.clone();
                        let activity_channel = channel.clone();
                        let retry_channel = channel.clone();
                        let progress_page_id = page.id.clone();
                        let retry_page_id = page.id.clone();
                        let result = generate_agent_page_to_disk(
                            &app,
                            db,
                            &params,
                            &slot,
                            &page,
                            &language,
                            &[],
                            &token,
                            Arc::new(move |content| {
                                send_wiki_event(
                                    &progress_channel,
                                    WikiGenerationEvent::Progress {
                                        page_id: progress_page_id.clone(),
                                        content,
                                    },
                                );
                            }),
                            Arc::new(move |text| {
                                send_wiki_event(
                                    &activity_channel,
                                    WikiGenerationEvent::ActivityBatch {
                                        activity_type: "tool".into(),
                                        items: vec![text],
                                    },
                                );
                            }),
                            Arc::new(move |notice: WikiRetryNotice| {
                                send_wiki_event(
                                    &retry_channel,
                                    WikiGenerationEvent::Retry {
                                        page_id: Some(retry_page_id.clone()),
                                        attempt: notice.attempt,
                                        max_attempts: notice.max_attempts,
                                        delay_seconds: notice.delay_seconds,
                                        reason: notice.reason,
                                    },
                                );
                            }),
                        )
                        .await;
                        let (status, error, duration_ms) = if token.is_cancelled() {
                            ("cancelled", None, None)
                        } else {
                            match result {
                                Ok(stats) => ("done", None, Some(stats.duration_ms)),
                                Err(error) => {
                                    let message = error.to_string();
                                    page_errors.lock().unwrap().push(error);
                                    ("failed", Some(message), None)
                                }
                            }
                        };
                        send_wiki_event(
                            &channel,
                            WikiGenerationEvent::Page {
                                page,
                                status: status.into(),
                                error,
                                duration_ms,
                            },
                        );
                    }
                })
                .await;
        }
    }

    if let Some(watch) = agent_cancel_watch.take() {
        watch.abort();
    }
    if let Some(slot) = &agent_slot {
        slot.cancel_all();
    }
    if run.token.is_cancelled() {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(());
    }
    let page_error = {
        let mut errors = page_errors.lock().unwrap();
        if errors.is_empty() {
            None
        } else {
            Some(errors.remove(0))
        }
    };
    if let Some(error) = page_error {
        return fail_wiki_generation(&on_event, &run.token, error);
    }
    let save_result = wiki::save_wiki_meta(
        app,
        request.project_path.clone(),
        wiki::WikiMeta {
            project_path: request.project_path,
            head_sha: context.head_sha,
            model: meta_model,
            language: request.language,
            status: "completed".into(),
            outline: pages,
            generator: Some(backend_id),
            ..Default::default()
        },
        Some(wiki::WikiCommitKind::Generate),
    );
    if let Err(error) = save_result {
        return fail_wiki_generation(&on_event, &run.token, error);
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "done".into(),
        },
    );
    Ok(())
}
