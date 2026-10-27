use super::*;

/// 增量更新的变更检测、受影响页面筛选、页面生成与 meta 推进全部在后端完成;
/// 生成始终使用内置 Agent,模型/思考强度从项目 Wiki 目录的 config.json 读取。
/// 自动更新不比较旧 Wiki 记录的生成器或模型,直接用当前项目配置重生成受影响
/// 页面;遇到旧 Wiki 或历史改写时仍静默跳过。手动更新遇到旧版三方 agent 后端
/// 生成的 Wiki(generator 不一致)时返回错误,由界面沿既有语义退化为整本重生成。
#[tauri::command]
pub async fn ai_update_wiki(
    app: AppHandle,
    db: State<'_, Db>,
    request: UpdateWikiRequest,
    on_event: Channel<WikiUpdateEvent>,
) -> AppResult<WikiUpdateResult> {
    let run = RegisteredRun::new(request.run_id.clone());
    let data_dir = crate::app_data_dir(&app)?;
    update_wiki_at(&data_dir, &db, request, &run.id, &run.token, &|event| {
        let _ = on_event.send(event);
    })
    .await
}

/// CLI(headless)增量更新:同步等待所有受影响页面生成完成。
pub(crate) async fn cli_update_wiki(
    data_dir: &std::path::Path,
    db: &Db,
    request: UpdateWikiRequest,
) -> AppResult<WikiUpdateResult> {
    let run_id = request.run_id.clone();
    let token = CancellationToken::new();
    update_wiki_at(data_dir, db, request, &run_id, &token, &|_| {}).await
}

async fn update_wiki_at(
    data_dir: &std::path::Path,
    db: &Db,
    request: UpdateWikiRequest,
    run_id: &str,
    cancel: &CancellationToken,
    on_event: &(dyn Fn(WikiUpdateEvent) + Send + Sync),
) -> AppResult<WikiUpdateResult> {
    let Some(data) = wiki::load_wiki_at(data_dir, &request.project_path) else {
        return Ok(WikiUpdateResult::default());
    };
    let config = wiki::load_wiki_config_at(data_dir, &request.project_path)?;
    let Some(from_sha) = data.meta.head_sha.clone() else {
        if request.automatic {
            return Ok(WikiUpdateResult::default());
        }
        return Err(AppError::coded(ErrorCode::GitCommandFailed, "no head sha"));
    };
    if should_reject_wiki_backend_change(
        data.meta.generator.as_deref(),
        "builtin",
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
    on_event(WikiUpdateEvent {
        completed: 0,
        total,
    });

    let mut generated_model = data.meta.model.clone();
    let mut generated_generator = data.meta.generator.clone();
    if !affected.is_empty() {
        generated_generator = Some("builtin".into());
        for (index, page) in affected.iter().enumerate() {
            generated_model = generate_builtin_page_to_disk(
                data_dir,
                db,
                run_id,
                &request.project_path,
                page,
                &request.language,
                &changed.files,
                config.model.as_deref(),
                config.thinking.as_deref(),
                cancel,
                Arc::new(|_| {}),
                Arc::new(|_| {}),
                Arc::new(|_| {}),
            )
            .await?;
            on_event(WikiUpdateEvent {
                completed: index + 1,
                total,
            });
        }
    }

    if let Some(head_sha) = changed.head_sha {
        wiki::save_wiki_meta_at(
            data_dir,
            &request.project_path,
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
