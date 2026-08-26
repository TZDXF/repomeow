use super::*;

/// 增量更新的变更检测、受影响页面筛选、页面生成与 meta 推进全部在后端完成。
/// 生成后端始终从项目 Wiki 目录的 config.json 读取。自动更新不比较旧 Wiki 记录的
/// 生成后端或模型，直接用当前项目配置重生成受影响页面；遇到旧 Wiki 或历史改写时仍
/// 静默跳过。手动更新遇到后端切换等不可增量情况时返回错误，由界面沿既有语义
/// 退化为整本重生成。
#[tauri::command]
pub async fn ai_update_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: UpdateWikiRequest,
    on_event: Channel<WikiUpdateEvent>,
) -> AppResult<WikiUpdateResult> {
    let run = RegisteredRun::new(request.run_id);
    let Some(data) = wiki::load_wiki(app.clone(), request.project_path.clone())? else {
        return Ok(WikiUpdateResult::default());
    };
    let backend = wiki::load_wiki_config_internal(&app, &request.project_path)?.backend;
    let Some(from_sha) = data.meta.head_sha.clone() else {
        if request.automatic {
            return Ok(WikiUpdateResult::default());
        }
        return Err(AppError::coded(ErrorCode::GitCommandFailed, "no head sha"));
    };
    let backend_id = wiki_backend_id(&backend);
    if should_reject_wiki_backend_change(
        data.meta.generator.as_deref(),
        &backend_id,
        request.automatic,
    ) {
        return Err(AppError::coded(
            ErrorCode::AiRequestFailed,
            "generator mismatch",
        ));
    }
    let changed = match wiki::wiki_changed_files(request.project_path.clone(), from_sha) {
        Ok(changed) => changed,
        Err(_) if request.automatic => return Ok(WikiUpdateResult::default()),
        Err(error) => return Err(error),
    };
    let changed_set: HashSet<&str> = changed.files.iter().map(String::as_str).collect();
    let affected: Vec<_> = data
        .meta
        .outline
        .iter()
        .filter(|page| {
            page.relevant_files
                .iter()
                .any(|file| changed_set.contains(file.as_str()))
        })
        .cloned()
        .collect();
    let total = affected.len();
    let _ = on_event.send(WikiUpdateEvent {
        completed: 0,
        total,
    });

    let mut generated_model = data.meta.model.clone();
    let mut generated_generator = data.meta.generator.clone();
    if !affected.is_empty() {
        generated_generator = Some(backend_id.clone());
        match &backend {
            WikiGenerationBackend::Builtin => {
                generated_model = sdk::load_config(&app).ai_model;
                for (index, page) in affected.iter().enumerate() {
                    generate_builtin_page_to_disk(
                        &app,
                        &db,
                        &request.project_path,
                        page,
                        &request.language,
                        &run.token,
                        |_| {},
                        |_| {},
                    )
                    .await?;
                    let _ = on_event.send(WikiUpdateEvent {
                        completed: index + 1,
                        total,
                    });
                }
            }
            WikiGenerationBackend::Agent { .. } => {
                let params = AgentSessionParams::from_backend(&backend, &request.project_path)
                    .expect("agent backend");
                let slot = AgentSessionSlot::default();
                let cancel_watch = watch_agent_cancel(run.token.clone(), slot.clone());
                // 与整本生成同款并发:出错即取消其余页面,最终上报第一个错误
                let first_error = Arc::new(Mutex::new(None::<AppError>));
                let model_cell = Arc::new(Mutex::new(generated_model.clone()));
                let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                stream::iter(affected.clone())
                    .for_each_concurrent(params.concurrency, |page| {
                        let app = app.clone();
                        let db = &db;
                        let params = params.clone();
                        let slot = slot.clone();
                        let token = run.token.clone();
                        let language = request.language.clone();
                        let changed_files = changed.files.clone();
                        let first_error = first_error.clone();
                        let model_cell = model_cell.clone();
                        let completed = completed.clone();
                        let on_event = on_event.clone();
                        async move {
                            if token.is_cancelled() {
                                return;
                            }
                            match generate_agent_page_to_disk(
                                &app,
                                db,
                                &params,
                                &slot,
                                &page,
                                &language,
                                &changed_files,
                                &token,
                                Arc::new(|_| {}),
                                Arc::new(|_| {}),
                                Arc::new(|_| {}),
                            )
                            .await
                            {
                                Ok(stats) => {
                                    *model_cell.lock().unwrap() = stats.usage_model;
                                }
                                Err(error) => {
                                    if !token.is_cancelled() {
                                        *first_error.lock().unwrap() = Some(error);
                                        // 取消其余页面(经 cancel watch 杀进行中会话)
                                        token.cancel();
                                    }
                                }
                            }
                            let done =
                                completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                            let _ = on_event.send(WikiUpdateEvent {
                                completed: done,
                                total,
                            });
                        }
                    })
                    .await;
                generated_model = model_cell.lock().unwrap().clone();
                slot.cancel_all();
                cancel_watch.abort();
                let first_error = first_error.lock().unwrap().take();
                if let Some(error) = first_error {
                    return Err(error);
                }
            }
        }
    }

    if let Some(head_sha) = changed.head_sha {
        wiki::save_wiki_meta(
            app,
            request.project_path.clone(),
            wiki::WikiMeta {
                head_sha: Some(head_sha),
                model: generated_model,
                generator: generated_generator,
                ..data.meta
            },
            Some(wiki::WikiCommitKind::Update),
        )?;
    }
    Ok(WikiUpdateResult {
        updated_page_ids: affected.into_iter().map(|page| page.id).collect(),
    })
}
