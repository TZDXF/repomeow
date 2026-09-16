use serde::Serialize;
use tauri::AppHandle;

use crate::cli::skills::{BUILTIN_SKILLS, EXECUTABLE_PLACEHOLDER};
use crate::commands::ai::{cli_upsert_builtin_skills, RlResult};
use crate::error::AppError;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSkillsInstallResult {
    /// 本次新导入资源库的技能名。
    pub installed: Vec<String>,
    /// 已存在、内容被刷新的技能名。
    pub updated: Vec<String>,
}

/// 将内置 CLI skill 一键导入资源库(按名称幂等,已存在则整体重建技能目录,
/// 以刷新其中的可执行文件路径);导入后在「项目 → AI 资源」部署到各 agent。
#[tauri::command]
pub async fn cli_install_builtin_skills(app: AppHandle) -> RlResult<CliSkillsInstallResult> {
    let executable = std::env::current_exe()
        .map_err(AppError::from)?
        .to_string_lossy()
        .into_owned();
    let skills: Vec<(String, String, Vec<(String, String)>)> = BUILTIN_SKILLS
        .iter()
        .map(|skill| {
            (
                skill.name.to_string(),
                skill.description.to_string(),
                skill
                    .files
                    .iter()
                    .map(|(path, content)| {
                        (
                            path.to_string(),
                            content.replace(EXECUTABLE_PLACEHOLDER, executable.as_str()),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    let (installed, updated) = cli_upsert_builtin_skills(&app, skills).await?;
    Ok(CliSkillsInstallResult { installed, updated })
}
