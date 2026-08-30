use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::ai::{prompts, sdk};
use crate::error::AppResult;
use crate::models::GitCommitInfo;

pub(crate) type AiConfig = sdk::AiConfig;

pub(crate) fn load_ai_config(data_dir: &PathBuf) -> AiConfig {
    crate::ai::catalog::legacy_ai_config(&crate::ai::catalog::load_ai_config_file_at(data_dir))
        .normalized()
}

/// 从 settings.json 读取界面语言(报告语言与其保持一致),默认 zh-CN
pub(crate) fn load_language(data_dir: &PathBuf) -> String {
    let path = data_dir.join("settings.json");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| {
            v.get("language")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "zh-CN".into())
}

pub(crate) fn default_schedule_name(is_weekly: bool, language: &str) -> &'static str {
    match (is_weekly, language) {
        (true, "en-US") => "Weekly Schedule",
        (false, "en-US") => "Daily Schedule",
        (true, _) => "周报定时任务",
        (false, _) => "日报定时任务",
    }
}

pub(crate) fn load_report_prompt(data_dir: &PathBuf, report_type: &str) -> String {
    let file = if report_type == "weekly" {
        "report-weekly.md"
    } else {
        "report.md"
    };
    fs::read_to_string(data_dir.join("prompts").join(file)).unwrap_or_default()
}

pub(crate) fn default_prompt(report_type: &str) -> &'static str {
    if report_type == "weekly" {
        prompts::DEFAULT_WEEKLY_REPORT_PROMPT
    } else {
        prompts::DEFAULT_REPORT_PROMPT
    }
}

pub(crate) async fn call_ai(
    config: &AiConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> AppResult<sdk::ChatOutput> {
    sdk::chat(config, Some(system_prompt), user_prompt, false, None, None).await
}

pub(crate) fn build_report_prompt(
    commits_by_project: &[(String, String, Vec<GitCommitInfo>)],
    range_label: &str,
    language: &str,
) -> String {
    let sections: Vec<String> = commits_by_project
        .iter()
        .map(|(name, desc, commits)| {
            let heading = if desc.is_empty() {
                format!("### {name}")
            } else {
                format!("### {name} — {desc}")
            };
            let lines: Vec<String> = commits
                .iter()
                .map(|c| format!("- [{}] {} ({}, {})", c.date, c.subject, c.hash, c.author))
                .collect();
            if lines.is_empty() {
                format!("{heading}\n(no commits)")
            } else {
                format!("{heading}\n{}", lines.join("\n"))
            }
        })
        .collect();

    let lang = if language == "zh-CN" {
        "中文"
    } else {
        "English"
    };
    format!(
        "Time range: {range_label}.\n\nCommit records:\n{}\n\nRespond in {lang}.",
        sections.join("\n\n")
    )
}
