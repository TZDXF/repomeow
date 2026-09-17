//! CLI `ai` / `prompt` 分组:AI 接入配置、模型探测、连接测试、用量统计与提示词管理。

use std::path::Path;

use serde_json::{json, Value};

use crate::ai::{catalog, sdk};
use crate::commands::{prompt, usage};

use super::util::{data_root_or_default, open_db, ToolFailure};

/// 读取 AI 接入配置(含 apiKey;文件缺失/损坏时自动播种,与应用行为一致)。
pub(super) fn ai_config_get_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let config = catalog::load_ai_config_file_at(&data_root);
    Ok(json!(config))
}

/// 从 JSON 文件全量覆盖 AI 接入配置(结构同 config-get 输出;原子写 + 引用归一化)。
pub(super) fn ai_config_save_impl(
    file: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let raw = std::fs::read_to_string(file).map_err(|error| {
        ToolFailure::new("ai_config_file_read_failed", "读取配置 JSON 文件失败")
            .with_detail(format!("{file}: {error}"))
    })?;
    let config: catalog::AiConfigFile = serde_json::from_str(&raw).map_err(|error| {
        ToolFailure::new("ai_config_invalid", "配置 JSON 格式无效").with_detail(error.to_string())
    })?;
    let data_root = data_root_or_default(data_root)?;
    catalog::save_ai_config_file_at(&data_root, &config)
        .map_err(|error| ToolFailure::from_app("保存 AI 配置失败", error))?;
    Ok(json!({ "saved": true }))
}

/// 内置厂商目录(含预置模型清单,apiKey 恒为空)。
pub(super) fn ai_builtin_providers_impl() -> Result<Value, ToolFailure> {
    Ok(json!({ "providers": catalog::builtin_config().providers }))
}

/// 列出默认模型所属厂商的可用模型(访问对应厂商 API)。
pub(super) async fn ai_models_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let config = sdk::load_config_at(&data_root, "default");
    let models = sdk::list_models(&config)
        .await
        .map_err(|error| ToolFailure::from_app("获取模型列表失败", error))?;
    Ok(json!({ "models": models }))
}

/// 测试默认模型连通性(发送一次性最小请求)。
pub(super) async fn ai_test_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let config = sdk::load_config_at(&data_root, "default");
    sdk::chat(
        &config,
        None,
        "Reply with the single word: ok",
        false,
        Some(8),
        None,
    )
    .await
    .map_err(|error| ToolFailure::from_app("AI 连接测试失败", error))?;
    Ok(json!({ "ok": true, "model": config.ai_model }))
}

pub(super) fn ai_usage_summary_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let summary = usage::usage_summary(&conn)
        .map_err(|error| ToolFailure::from_app("查询 AI 用量汇总失败", error))?;
    Ok(json!(summary))
}

pub(super) fn ai_usage_log_impl(
    offset: i64,
    limit: i64,
    task_type: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let entries = usage::list_usage_rows(&conn, offset, limit, task_type)
        .map_err(|error| ToolFailure::from_app("查询 AI 用量日志失败", error))?;
    Ok(json!({ "entries": entries }))
}

pub(super) fn ai_usage_clear_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let deleted = usage::clear_usage_rows(&conn)
        .map_err(|error| ToolFailure::from_app("清空 AI 用量日志失败", error))?;
    Ok(json!({ "deleted": deleted }))
}

// ── AI 提示词 ─────────────────────────────────────────────────────────

/// 读取用户自定义提示词;空字符串表示该项使用内置默认模板。
pub(super) fn prompts_get_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let dir = prompt::prompts_dir_in(&data_root);
    Ok(json!(prompt::read_prompts_in(&dir)))
}

/// 读取内置默认提示词模板(只读预览)。
pub(super) fn prompts_default_impl() -> Result<Value, ToolFailure> {
    Ok(json!(prompt::AiPrompts {
        commit: crate::ai::prompts::DEFAULT_COMMIT_PROMPT.trim().to_string(),
        report: crate::ai::prompts::DEFAULT_REPORT_PROMPT.trim().to_string(),
        report_weekly: crate::ai::prompts::DEFAULT_WEEKLY_REPORT_PROMPT
            .trim()
            .to_string(),
    }))
}

/// 按文件内容设置提示词;未提供的项保持现状,传 --clear 的项恢复默认。
pub(super) fn prompts_set_impl(
    commit_file: Option<&str>,
    report_file: Option<&str>,
    report_weekly_file: Option<&str>,
    clear: bool,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let dir = prompt::prompts_dir_in(&data_root);
    let mut prompts = prompt::read_prompts_in(&dir);
    let read_file = |file: &str| -> Result<String, ToolFailure> {
        std::fs::read_to_string(file).map_err(|error| {
            ToolFailure::new("prompt_file_read_failed", "读取提示词文件失败")
                .with_detail(format!("{file}: {error}"))
        })
    };
    if let Some(file) = commit_file {
        prompts.commit = read_file(file)?;
    }
    if let Some(file) = report_file {
        prompts.report = read_file(file)?;
    }
    if let Some(file) = report_weekly_file {
        prompts.report_weekly = read_file(file)?;
    }
    if clear {
        // 仅清空本次未显式指定的项
        if commit_file.is_none() {
            prompts.commit = String::new();
        }
        if report_file.is_none() {
            prompts.report = String::new();
        }
        if report_weekly_file.is_none() {
            prompts.report_weekly = String::new();
        }
    }
    prompt::write_prompts_in(&dir, &prompts)
        .map_err(|error| ToolFailure::from_app("保存提示词失败", error))?;
    Ok(json!(prompts))
}
