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
    let run = RegisteredRun::new(request.run_id);
    let config = wiki::load_wiki_config_internal(&app, &request.project_path)?;
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

    send_wiki_event(
        &on_event,
        WikiGenerationEvent::Phase {
            phase: "outlining".into(),
        },
    );
    let pages_result = generate_builtin_outline_pages(
        &app,
        &db,
        &context,
        &request.project_path,
        &request.project_name,
        &request.language,
        config.model.as_deref(),
        config.thinking.as_deref(),
        &run.token,
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
        send_wiki_event(
            &on_event,
            WikiGenerationEvent::Phase {
                phase: "cancelled".into(),
            },
        );
        return Ok(());
    }

    if let Err(error) = wiki::begin_wiki(app.clone(), request.project_path.clone()) {
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
    let run_id = run.id.clone();
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
            let app = app.clone();
            let db = &db;
            let project_path = request.project_path.clone();
            let language = request.language.clone();
            let token = run.token.clone();
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
                    &app,
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
            generator: Some("builtin".into()),
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
