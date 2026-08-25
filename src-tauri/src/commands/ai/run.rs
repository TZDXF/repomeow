use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::ai::sdk::{self, AiConfig, ChatOutput};
use crate::commands::usage::insert_usage_row;
use crate::db::Db;
use crate::error::AppResult;
use crate::models::AiUsageRecord;
use crate::time_util::now_ts;

static AI_RUNS: OnceLock<Mutex<HashMap<String, CancellationToken>>> = OnceLock::new();

fn ai_runs() -> &'static Mutex<HashMap<String, CancellationToken>> {
    AI_RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) struct RegisteredRun {
    pub(super) id: String,
    pub(super) token: CancellationToken,
}

impl RegisteredRun {
    pub(super) fn new(id: String) -> Self {
        let token = CancellationToken::new();
        ai_runs().lock().unwrap().insert(id.clone(), token.clone());
        Self { id, token }
    }
}

impl Drop for RegisteredRun {
    fn drop(&mut self) {
        ai_runs().lock().unwrap().remove(&self.id);
    }
}

pub(super) fn record_usage(
    db: &Db,
    task_type: &str,
    model: &str,
    output: &ChatOutput,
    duration_ms: i64,
) {
    let usage = output.usage.as_ref();
    let record = AiUsageRecord {
        task_type: task_type.to_string(),
        model: model.to_string(),
        input_tokens: usage.and_then(|value| value.input_tokens),
        output_tokens: usage.and_then(|value| value.output_tokens),
        total_tokens: usage.and_then(|value| value.total_tokens),
        duration_ms: Some(duration_ms),
        cached_tokens: usage.and_then(|value| value.cached_tokens),
    };
    if let Ok(conn) = db.0.lock() {
        let _ = insert_usage_row(&conn, &record, now_ts());
    }
}

#[tauri::command]
pub async fn ai_list_models(config: AiConfig) -> AppResult<Vec<String>> {
    sdk::list_models(&config.normalized()).await
}

#[tauri::command]
pub async fn ai_test_connection(app: AppHandle) -> AppResult<()> {
    let config = sdk::load_config(&app);
    sdk::chat(
        &config,
        None,
        "Reply with the single word: ok",
        false,
        Some(8),
        None,
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub fn ai_cancel_run(run_id: String) -> AppResult<()> {
    if let Some(token) = ai_runs().lock().unwrap().get(&run_id) {
        token.cancel();
    }
    Ok(())
}
