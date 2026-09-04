//! Markdown 文档翻译(AI 面板预览抽屉的「翻译」按钮)。
//! 走设置页默认模型;提示词固定不开放自定义;用量以 translate 类型落库。

use std::time::Instant;

use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::ai::prompts::{fixed_system_prompt, DEFAULT_TRANSLATE_PROMPT};
use crate::ai::sdk;
use crate::db::Db;
use crate::error::AppResult;

use super::run::{record_usage, RegisteredRun};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateMarkdownRequest {
    /// 待翻译的 Markdown 全文。
    text: String,
    /// 目标语言(zh-CN / en-US,与前端界面语言一致)。
    language: String,
    /// 取消句柄:前端生成独立 runId,取消时经 ai_cancel_run 置位;缺省表示不可取消。
    #[serde(default)]
    run_id: Option<String>,
}

/// 翻译一段 Markdown 文档;取消后返回 None(与 commit/report 一致)。
#[tauri::command]
pub async fn ai_translate_markdown(
    app: AppHandle,
    db: State<'_, Db>,
    request: TranslateMarkdownRequest,
) -> AppResult<Option<String>> {
    let run = request
        .run_id
        .as_deref()
        .map(|id| RegisteredRun::new(id.to_string()));
    let system_prompt = fixed_system_prompt(DEFAULT_TRANSLATE_PROMPT, &request.language);
    let config = sdk::load_config(&app);
    let started = Instant::now();
    let output = match sdk::chat(
        &config,
        Some(&system_prompt),
        &request.text,
        false,
        None,
        run.as_ref().map(|run| &run.token),
    )
    .await
    {
        Ok(output) => output,
        Err(_) if run.as_ref().is_some_and(|run| run.token.is_cancelled()) => return Ok(None),
        Err(error) => return Err(error),
    };
    record_usage(
        &db,
        "translate",
        &config.ai_model,
        &output,
        started.elapsed().as_millis() as i64,
    );
    Ok(Some(output.text))
}
