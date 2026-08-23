use std::fs;

use tauri::AppHandle;

use crate::app_data_dir;

pub const DEFAULT_COMMIT_PROMPT: &str = include_str!("prompts/commit.md");
pub const DEFAULT_REPORT_PROMPT: &str = include_str!("prompts/report-daily.md");
pub const DEFAULT_WEEKLY_REPORT_PROMPT: &str = include_str!("prompts/report-weekly.md");
pub const DEFAULT_WIKI_OUTLINE_PROMPT: &str = include_str!("prompts/wiki-outline.md");
pub const DEFAULT_WIKI_PAGE_PROMPT: &str = include_str!("prompts/wiki-page.md");
pub const AGENT_WIKI_OUTLINE_PROMPT: &str = include_str!("prompts/wiki-agent-outline.md");
pub const AGENT_WIKI_PAGE_PROMPT: &str = include_str!("prompts/wiki-agent-page.md");

pub fn language_name(language: &str) -> &'static str {
    if language == "zh-CN" {
        "中文"
    } else {
        "English"
    }
}

pub fn fixed_system_prompt(template: &str, language: &str) -> String {
    format!(
        "{}\n\nRespond in {}.",
        template.trim(),
        language_name(language)
    )
}

pub fn effective_system_prompt(
    app: &AppHandle,
    custom_file: &str,
    fallback: &str,
    language: &str,
) -> String {
    let custom = app_data_dir(app)
        .ok()
        .and_then(|dir| fs::read_to_string(dir.join("prompts").join(custom_file)).ok())
        .unwrap_or_default();
    fixed_system_prompt(
        if custom.trim().is_empty() {
            fallback
        } else {
            custom.trim()
        },
        language,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_instruction_is_appended_once() {
        assert_eq!(
            fixed_system_prompt("system", "zh-CN"),
            "system\n\nRespond in 中文."
        );
        assert_eq!(
            fixed_system_prompt("system\n", "en-US"),
            "system\n\nRespond in English."
        );
    }

    #[test]
    fn all_embedded_prompts_are_present() {
        for prompt in [
            DEFAULT_COMMIT_PROMPT,
            DEFAULT_REPORT_PROMPT,
            DEFAULT_WEEKLY_REPORT_PROMPT,
            DEFAULT_WIKI_OUTLINE_PROMPT,
            DEFAULT_WIKI_PAGE_PROMPT,
            AGENT_WIKI_OUTLINE_PROMPT,
            AGENT_WIKI_PAGE_PROMPT,
        ] {
            assert!(!prompt.trim().is_empty());
        }
    }
}
