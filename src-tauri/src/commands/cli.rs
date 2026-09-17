use serde::Serialize;
use tauri::AppHandle;

use crate::cli::skills::BUILTIN_SKILLS;
use crate::commands::ai::{cli_upsert_builtin_skills, RlResult};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSkillsInstallResult {
    /// 本次新导入资源库的技能名。
    pub installed: Vec<String>,
    /// 已存在、内容被刷新的技能名。
    pub updated: Vec<String>,
}

/// 将内置 CLI skill 一键导入资源库(按名称幂等,已存在则整体重建技能目录,
/// 以同步最新技能内容);导入后在「项目 → AI 资源」部署到各 agent。
#[tauri::command]
pub async fn cli_install_builtin_skills(app: AppHandle) -> RlResult<CliSkillsInstallResult> {
    let skills: Vec<(String, String, Vec<(String, String)>)> = BUILTIN_SKILLS
        .iter()
        .map(|skill| {
            (
                skill.name.to_string(),
                skill.description.to_string(),
                skill
                    .files
                    .iter()
                    .map(|(path, content)| (path.to_string(), content.to_string()))
                    .collect(),
            )
        })
        .collect();
    let (installed, updated) = cli_upsert_builtin_skills(&app, skills).await?;
    Ok(CliSkillsInstallResult { installed, updated })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliPathStatus {
    supported: bool,
    directory: String,
    added: bool,
}

#[cfg(windows)]
fn matches_path(entry: &str, directory: &str) -> bool {
    crate::path_util::clean_str(entry.trim().trim_matches('"'))
        .eq_ignore_ascii_case(&crate::path_util::clean_str(directory))
}

#[cfg(windows)]
fn change_path(value: &str, directory: &str, enabled: bool) -> String {
    if enabled {
        if value.split(';').any(|p| matches_path(p, directory)) {
            return value.into();
        }
        let separator = if value.is_empty() || value.ends_with(';') {
            ""
        } else {
            ";"
        };
        format!("{value}{separator}{directory}")
    } else {
        value
            .split(';')
            .filter(|p| !matches_path(p, directory))
            .collect::<Vec<_>>()
            .join(";")
    }
}

fn user_path(enabled: Option<bool>) -> crate::error::AppResult<CliPathStatus> {
    let exe = std::env::current_exe()?;
    let directory = exe
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing executable parent"))?
        .to_string_lossy()
        .into_owned();
    #[cfg(windows)]
    {
        use winreg::{enums::*, types::FromRegValue, RegKey, RegValue};
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
            "Environment",
            if enabled.is_some() {
                KEY_READ | KEY_WRITE
            } else {
                KEY_READ
            },
        )?;
        let (value, kind) = match key.get_raw_value("Path") {
            Ok(raw) if raw.vtype == REG_SZ || raw.vtype == REG_EXPAND_SZ => {
                (String::from_reg_value(&raw)?, raw.vtype)
            }
            Ok(_) => return Err(std::io::Error::other("User Path is not a string").into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), REG_EXPAND_SZ),
            Err(e) => return Err(e.into()),
        };
        let updated = enabled.map_or_else(|| value.clone(), |e| change_path(&value, &directory, e));
        if value != updated {
            let bytes = updated
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            key.set_raw_value("Path", &RegValue { bytes, vtype: kind })?;
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            let environment: Vec<u16> = "Environment\0".encode_utf16().collect();
            // 已运行的终端仍需重启；通知 Explorer 读取更新后的用户环境。
            unsafe {
                SendMessageTimeoutW(
                    HWND_BROADCAST,
                    WM_SETTINGCHANGE,
                    0,
                    environment.as_ptr() as isize,
                    SMTO_ABORTIFHUNG,
                    1000,
                    std::ptr::null_mut(),
                );
            }
        }
        Ok(CliPathStatus {
            supported: true,
            added: updated.split(';').any(|p| matches_path(p, &directory)),
            directory,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Ok(CliPathStatus {
            supported: false,
            directory,
            added: false,
        })
    }
}

#[tauri::command]
pub async fn cli_get_path_status() -> crate::error::AppResult<CliPathStatus> {
    user_path(None)
}

#[tauri::command]
pub async fn cli_set_user_path(enabled: bool) -> crate::error::AppResult<CliPathStatus> {
    user_path(Some(enabled))
}

#[cfg(all(test, windows))]
mod path_tests {
    use super::*;
    #[test]
    fn path_changes_preserve_other_entries_and_are_idempotent() {
        let original = r"%USERPROFILE%\bin;C:\Tools";
        let added = change_path(original, r"C:\RepoMeow", true);
        assert_eq!(change_path(&added, "c:/repomeow/", true), added);
        assert_eq!(change_path(&added, r"C:\RepoMeow", false), original);
        assert_eq!(change_path("", r"C:\RepoMeow", false), "");
        assert_eq!(
            change_path(
                r#"C:\RepoMeow-old;"c:/repomeow/";C:\Other"#,
                r"C:\RepoMeow",
                false
            ),
            r"C:\RepoMeow-old;C:\Other"
        );
    }
}
