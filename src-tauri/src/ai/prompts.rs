use std::fs;

use tauri::AppHandle;

use crate::app_data_dir;

pub const DEFAULT_COMMIT_PROMPT: &str = include_str!("prompts/commit.md");
pub const DEFAULT_REPORT_PROMPT: &str = include_str!("prompts/report-daily.md");
pub const DEFAULT_WEEKLY_REPORT_PROMPT: &str = include_str!("prompts/report-weekly.md");
pub const AGENT_WIKI_OUTLINE_PROMPT: &str = include_str!("prompts/wiki-agent-outline.md");
pub const AGENT_WIKI_PAGE_PROMPT: &str = include_str!("prompts/wiki-agent-page.md");
pub const BUILTIN_AGENT_WIKI_PAGE_PROMPT: &str = include_str!("prompts/wiki-builtin-agent-page.md");

pub fn language_name(language: &str) -> &'static str {
    if language == "zh-CN" {
        "中文"
    } else {
        "English"
    }
}

pub fn fixed_system_prompt(template: &str, language: &str) -> String {
    format!(
        "{}\n\n# Output language (mandatory)\n- Write the response in {}.\n- Keep code identifiers, file paths, URLs, and conventional keywords unchanged.",
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
            "system\n\n# Output language (mandatory)\n- Write the response in 中文.\n- Keep code identifiers, file paths, URLs, and conventional keywords unchanged."
        );
        assert_eq!(
            fixed_system_prompt("system\n", "en-US"),
            "system\n\n# Output language (mandatory)\n- Write the response in English.\n- Keep code identifiers, file paths, URLs, and conventional keywords unchanged."
        );
    }

    #[test]
    fn all_embedded_prompts_are_present() {
        for prompt in [
            DEFAULT_COMMIT_PROMPT,
            DEFAULT_REPORT_PROMPT,
            DEFAULT_WEEKLY_REPORT_PROMPT,
            AGENT_WIKI_OUTLINE_PROMPT,
            AGENT_WIKI_PAGE_PROMPT,
            BUILTIN_AGENT_WIKI_PAGE_PROMPT,
        ] {
            assert!(!prompt.trim().is_empty());
        }
    }

    #[test]
    fn agent_wiki_prompts_require_silent_self_validation() {
        for prompt in [AGENT_WIKI_OUTLINE_PROMPT, AGENT_WIKI_PAGE_PROMPT] {
            assert!(prompt.contains("If any criterion fails, revise the draft"));
            assert!(prompt.contains("the application owns persistence"));
            // 工具预算与禁令:限制探索次数,禁止跑命令/构建/测试
            assert!(prompt.contains("Never run shell commands"));
        }
        assert!(BUILTIN_AGENT_WIKI_PAGE_PROMPT.contains("Never run shell commands"));

        assert!(AGENT_WIKI_OUTLINE_PROMPT.contains("Output ONLY one complete JSON object"));
        assert!(AGENT_WIKI_OUTLINE_PROMPT.contains("\"relevantFiles\""));
        assert!(!AGENT_WIKI_OUTLINE_PROMPT.contains("<wiki_structure>"));
        assert!(
            AGENT_WIKI_OUTLINE_PROMPT.contains("Explore the repository and use tools silently.")
        );
        assert!(AGENT_WIKI_OUTLINE_PROMPT.contains("at most 20 additional files"));
        assert!(AGENT_WIKI_OUTLINE_PROMPT.contains("The first non-whitespace character is"));
        // 页面生成是混合模式:相关文件全文已喂入,只允许少量补充读取
        assert!(AGENT_WIKI_PAGE_PROMPT.contains("at most 5 additional repository files"));
        assert!(AGENT_WIKI_PAGE_PROMPT.contains("`N: `"));
        assert!(AGENT_WIKI_PAGE_PROMPT.contains("The first non-whitespace characters are"));
        assert!(AGENT_WIKI_PAGE_PROMPT.contains("Nothing — including an acknowledgement"));
        assert!(BUILTIN_AGENT_WIKI_PAGE_PROMPT.contains("exactly the writable draft path"));
        assert!(BUILTIN_AGENT_WIKI_PAGE_PROMPT.contains("Use write for a complete replacement"));
        assert!(BUILTIN_AGENT_WIKI_PAGE_PROMPT.contains("Never run shell commands"));
    }
}
