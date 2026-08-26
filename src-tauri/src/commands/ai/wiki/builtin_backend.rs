use super::*;

pub(super) async fn generate_builtin_outline_pages(
    app: &AppHandle,
    db: &Db,
    context: &wiki::WikiContext,
    project_name: &str,
    language: &str,
    cancel: &CancellationToken,
    on_retry: impl Fn(WikiRetryNotice),
) -> AppResult<Vec<wiki::WikiOutlinePage>> {
    let config = sdk::load_config(app);
    let system = fixed_system_prompt(DEFAULT_WIKI_OUTLINE_PROMPT, language);
    let original_prompt = wiki_outline_user_prompt(context, project_name);
    let mut prompt = original_prompt.clone();
    let valid_files: HashSet<String> = context.paths.iter().cloned().collect();
    let mut last_error = "wiki outline JSON was not generated".to_string();
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        let started = Instant::now();
        let output = match sdk::stream_chat(&config, &system, &prompt, true, cancel, |_| {}).await {
            Ok(output) => output,
            Err(error)
                if !cancel.is_cancelled()
                    && attempt < MAX_ATTEMPTS
                    && error.is_retryable_ai_error() =>
            {
                let notice = retry_notice(&error, attempt, MAX_ATTEMPTS);
                on_retry(notice.clone());
                wait_for_wiki_retry(cancel, &notice).await?;
                continue;
            }
            Err(error) => return Err(error),
        };
        record_usage(
            db,
            "wiki",
            &config.ai_model,
            &output,
            started.elapsed().as_millis() as i64,
        );
        match crate::ai::wiki_outline::parse_outline(&output.text, &valid_files) {
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

pub(super) async fn generate_builtin_page_to_disk(
    app: &AppHandle,
    db: &Db,
    project_path: &str,
    page: &wiki::WikiOutlinePage,
    language: &str,
    cancel: &CancellationToken,
    on_progress: impl Fn(&str),
    on_retry: impl Fn(WikiRetryNotice),
) -> AppResult<()> {
    let config = sdk::load_config(app);
    let system = fixed_system_prompt(DEFAULT_WIKI_PAGE_PROMPT, language);
    let files = wiki::read_wiki_files_in(project_path, &page.relevant_files)?;
    let prompt = wiki_page_user_prompt(page, &files);
    let mut last_error = None;
    const MAX_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        on_progress("");
        let started = Instant::now();
        match sdk::stream_chat(&config, &system, &prompt, true, cancel, |text| {
            on_progress(text)
        })
        .await
        {
            Ok(output) => {
                record_usage(
                    db,
                    "wiki",
                    &config.ai_model,
                    &output,
                    started.elapsed().as_millis() as i64,
                );
                wiki::save_wiki_page_internal(app, project_path, &page.file, &output.text)?;
                return Ok(());
            }
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => {
                if attempt < MAX_ATTEMPTS && error.is_retryable_ai_error() {
                    let notice = retry_notice(&error, attempt, MAX_ATTEMPTS);
                    on_retry(notice.clone());
                    wait_for_wiki_retry(cancel, &notice).await?;
                }
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AppError::coded(ErrorCode::AiRequestFailed, "wiki page generation failed")
    }))
}
