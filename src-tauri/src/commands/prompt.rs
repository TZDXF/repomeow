use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::commands::open;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::APP_DATA_DIR_NAME;

const PROMPTS_DIR_NAME: &str = "prompts";
const COMMIT_PROMPT_FILE: &str = "commit.md";
const REPORT_PROMPT_FILE: &str = "report.md";
const REPORT_WEEKLY_PROMPT_FILE: &str = "report-weekly.md";
const WIKI_OUTLINE_PROMPT_FILE: &str = "wiki-outline.md";
const WIKI_PAGE_PROMPT_FILE: &str = "wiki-page.md";

/// 用户自定义 AI 提示词(存为 ~/.repomeow/prompts/*.md);空字符串表示使用内置默认模板
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPrompts {
    pub commit: String,
    pub report: String,
    pub report_weekly: String,
    /// wiki 大纲(结构)生成提示词
    pub wiki_outline: String,
    /// wiki 单页内容生成提示词
    pub wiki_page: String,
}

fn prompts_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    Ok(home.join(APP_DATA_DIR_NAME).join(PROMPTS_DIR_NAME))
}

/// 读取单个提示词文件;不存在或读取失败时返回空串(前端回退默认模板)
fn read_prompt(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap_or_default()
}

/// 写入单个提示词文件;内容为空时删除文件(即恢复默认)
fn write_prompt(dir: &Path, file: &str, content: &str) -> AppResult<()> {
    let path = dir.join(file);
    if content.trim().is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    } else {
        fs::write(&path, content)?;
        Ok(())
    }
}

#[tauri::command]
pub fn get_ai_prompts(app: AppHandle) -> AppResult<AiPrompts> {
    let dir = prompts_dir(&app)?;
    Ok(AiPrompts {
        commit: read_prompt(&dir, COMMIT_PROMPT_FILE),
        report: read_prompt(&dir, REPORT_PROMPT_FILE),
        report_weekly: read_prompt(&dir, REPORT_WEEKLY_PROMPT_FILE),
        wiki_outline: read_prompt(&dir, WIKI_OUTLINE_PROMPT_FILE),
        wiki_page: read_prompt(&dir, WIKI_PAGE_PROMPT_FILE),
    })
}

#[tauri::command]
pub fn set_ai_prompts(app: AppHandle, prompts: AiPrompts) -> AppResult<()> {
    let dir = prompts_dir(&app)?;
    fs::create_dir_all(&dir)?;
    write_prompt(&dir, COMMIT_PROMPT_FILE, &prompts.commit)?;
    write_prompt(&dir, REPORT_PROMPT_FILE, &prompts.report)?;
    write_prompt(&dir, REPORT_WEEKLY_PROMPT_FILE, &prompts.report_weekly)?;
    write_prompt(&dir, WIKI_OUTLINE_PROMPT_FILE, &prompts.wiki_outline)?;
    write_prompt(&dir, WIKI_PAGE_PROMPT_FILE, &prompts.wiki_page)?;
    Ok(())
}

#[tauri::command]
pub fn open_prompts_dir(app: AppHandle) -> AppResult<()> {
    let dir = prompts_dir(&app)?;
    fs::create_dir_all(&dir)?;
    open::open_explorer(&dir.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_write_read_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-prompts-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();

        // 文件不存在时读取返回空串
        assert_eq!(read_prompt(&dir, COMMIT_PROMPT_FILE), "");

        write_prompt(&dir, COMMIT_PROMPT_FILE, "自定义提示词").unwrap();
        assert_eq!(read_prompt(&dir, COMMIT_PROMPT_FILE), "自定义提示词");

        // 写空白内容 = 删除文件,再次读取回到空串
        write_prompt(&dir, COMMIT_PROMPT_FILE, "   ").unwrap();
        assert_eq!(read_prompt(&dir, COMMIT_PROMPT_FILE), "");
        assert!(!dir.join(COMMIT_PROMPT_FILE).exists());

        // 删除不存在的文件不报错(幂等)
        write_prompt(&dir, COMMIT_PROMPT_FILE, "").unwrap();

        fs::remove_dir_all(&dir).ok();
    }
}
