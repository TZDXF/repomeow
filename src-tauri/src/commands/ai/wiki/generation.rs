use super::*;

/// 整本 Wiki 的收集、大纲、并发页生成、重试与最终落盘全部在后端执行;
/// 生成始终使用内置 Agent(模型/思考强度/并发取项目 Wiki 配置)。
#[tauri::command]
pub async fn ai_generate_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: GenerateWikiRequest,
    on_event: Channel<WikiGenerationEvent>,
) -> AppResult<()> {
    let run = RegisteredRun::new(request.run_id.clone());
    let data_dir = crate::app_data_dir(&app)?;
    let sink: WikiGenerationEventSink = Arc::new(move |event| {
        let _ = on_event.send(event);
    });
    generate_wiki_at(&data_dir, &db, request, &run.id, &run.token, sink)
        .await
        .map(|_| ())
}

/// CLI(headless)整本生成:与桌面端共用完整管线,同步等待完成后返回页面 id。
pub(crate) async fn cli_generate_wiki(
    data_dir: &std::path::Path,
    db: &Db,
    request: GenerateWikiRequest,
) -> AppResult<Vec<String>> {
    let run_id = request.run_id.clone();
    let token = CancellationToken::new();
    generate_wiki_at(data_dir, db, request, &run_id, &token, Arc::new(|_| {})).await
}

async fn generate_wiki_at(
    data_dir: &std::path::Path,
    db: &Db,
    request: GenerateWikiRequest,
    run_id: &str,
    cancel: &CancellationToken,
    on_event: WikiGenerationEventSink,
) -> AppResult<Vec<String>> {
    let config = wiki::load_wiki_config_at(data_dir, &request.project_path)?;
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "collecting".into(),
        },
    );
    let context = match wiki::collect_wiki_context(request.project_path.clone()) {
        Ok(context) => context,
        Err(error) => return fail_wiki_generation(&on_event, cancel, error),
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

    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "outlining".into(),
        },
    );
    let pages_result = generate_builtin_outline_pages(
        data_dir,
        db,
        &context,
        &request.project_path,
        &request.project_name,
        &request.language,
        config.model.as_deref(),
        config.thinking.as_deref(),
        cancel,
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
            Arc::new(move |notice| {
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

    let (pages, meta_model) = match pages_result {
        Ok(value) => value,
        Err(error) => {
            let phase = if cancel.is_cancelled() {
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
    if cancel.is_cancelled() {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(Vec::new());
    }

    if let Err(error) = wiki::begin_wiki_at(data_dir, &request.project_path) {
        return fail_wiki_generation(&on_event, cancel, error);
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
    let run_id = run_id.to_string();
    // 页面并发:项目配置优先,未配置沿用设置页全局 AI 并发
    let page_concurrency = config
        .concurrency
        .filter(|value| *value > 0)
        .unwrap_or(request.concurrency)
        .clamp(1, 8);
    let model = config.model.clone();
    let thinking = config.thinking.clone();
    stream::iter(pages.clone())
        .for_each_concurrent(page_concurrency, |page| {
            let data_dir = data_dir.to_path_buf();
            let db = &db;
            let project_path = request.project_path.clone();
            let language = request.language.clone();
            let token = cancel.clone();
            let run_id = run_id.clone();
            let channel = on_event.clone();
            let page_errors = page_errors.clone();
            let model = model.clone();
            let thinking = thinking.clone();
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
                let activity_channel = channel.clone();
                let result = generate_builtin_page_to_disk(
                    &data_dir,
                    db,
                    &run_id,
                    &project_path,
                    &page,
                    &language,
                    &[],
                    model.as_deref(),
                    thinking.as_deref(),
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
                    Arc::new(move |notice| {
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
                let (status, error) = if token.is_cancelled() {
                    ("cancelled", None)
                } else {
                    match result {
                        Ok(_) => ("done", None),
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

    if cancel.is_cancelled() {
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(Vec::new());
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
        return fail_wiki_generation(&on_event, cancel, error);
    }
    let page_ids = pages.iter().map(|page| page.id.clone()).collect::<Vec<_>>();
    let save_result = wiki::save_wiki_meta_at(
        data_dir,
        &request.project_path,
        wiki::WikiMeta {
            project_path: request.project_path.clone(),
            head_sha: context.head_sha,
            model: meta_model,
            language: request.language,
            status: "completed".into(),
            outline: pages,
            generator: Some("builtin".into()),
            ..Default::default()
        },
        Some(wiki::WikiCommitKind::Generate),
    );
    if let Err(error) = save_result {
        return fail_wiki_generation(&on_event, cancel, error);
    }
    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "done".into(),
        },
    );
    Ok(page_ids)
}
