use std::fs;
use std::path::Path;

use tauri::AppHandle;

use crate::commands::open;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;

use super::paths::{head_sha, wiki_dir};
use super::snapshot::{commit_message, commit_wiki_in};
use super::types::{
    WikiCommitKind, WikiData, WikiGenerationConfig, WikiMeta, WikiPageData, CONFIG_VERSION,
};

pub(super) const CONFIG_FILE: &str = "config.json";
pub(super) const META_FILE: &str = "meta.json";
pub(super) const PAGES_DIR: &str = "pages";
pub(super) const META_VERSION: u32 = 1;

fn valid_page_file(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".md")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

pub(super) fn save_page_in(dir: &Path, file_name: &str, content: &str) -> AppResult<()> {
    if !valid_page_file(file_name) {
        return Err(AppError::coded(ErrorCode::InvalidPath, file_name));
    }
    let pages = dir.join(PAGES_DIR);
    fs::create_dir_all(&pages)?;
    let target = pages.join(file_name);
    let tmp = pages.join(format!("{file_name}.tmp"));
    fs::write(&tmp, content)?;
    fs::rename(&tmp, &target)?;
    Ok(())
}

pub(super) fn save_meta_in(dir: &Path, mut meta: WikiMeta) -> AppResult<()> {
    meta.version = META_VERSION;
    meta.generated_at = chrono::Utc::now().to_rfc3339();
    fs::create_dir_all(dir)?;
    let target = dir.join(META_FILE);
    let tmp = dir.join(format!("{META_FILE}.tmp"));
    let json = serde_json::to_string_pretty(&meta)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &target)?;
    Ok(())
}

pub(super) fn save_config_in(dir: &Path, mut config: WikiGenerationConfig) -> AppResult<()> {
    config.version = CONFIG_VERSION;
    fs::create_dir_all(dir)?;
    let target = dir.join(CONFIG_FILE);
    let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &target)?;
    Ok(())
}

pub(super) fn load_config_in(dir: &Path) -> AppResult<WikiGenerationConfig> {
    let path = dir.join(CONFIG_FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WikiGenerationConfig::default());
        }
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&raw).map_err(|error| {
        AppError::coded(
            ErrorCode::IoError,
            format!("{}: {error}", path.to_string_lossy()),
        )
    })
}

fn legacy_wiki_config(app: &AppHandle) -> WikiGenerationConfig {
    let model = crate::tray::read_setting_string(app, "wikiAgentModel").filter(|v| !v.is_empty());
    let thinking =
        crate::tray::read_setting_string(app, "wikiAgentThinking").filter(|v| !v.is_empty());
    let backend = match crate::tray::read_setting_string(app, "wikiGenBackend").as_deref() {
        None | Some("") | Some("builtin") => crate::commands::ai::WikiGenerationBackend::Builtin,
        Some("custom") => crate::commands::ai::WikiGenerationBackend::Agent {
            agent_id: None,
            custom_command: crate::tray::read_setting_string(app, "wikiAgentCustomCommand")
                .filter(|v| !v.is_empty()),
            model,
            thinking,
            concurrency: None,
        },
        Some(agent_id) => crate::commands::ai::WikiGenerationBackend::Agent {
            agent_id: Some(agent_id.to_string()),
            custom_command: None,
            model,
            thinking,
            concurrency: None,
        },
    };
    WikiGenerationConfig {
        version: CONFIG_VERSION,
        backend,
    }
}

pub(super) fn load_wiki_in(dir: &Path) -> Option<(WikiMeta, Vec<WikiPageData>)> {
    let raw = fs::read_to_string(dir.join(META_FILE)).ok()?;
    let meta: WikiMeta = serde_json::from_str(&raw).ok()?;
    if meta.status != "completed" {
        return None;
    }
    let pages = meta
        .outline
        .iter()
        .map(|page| WikiPageData {
            id: page.id.clone(),
            file: page.file.clone(),
            title: page.title.clone(),
            section: page.section.clone(),
            importance: page.importance.clone(),
            relevant_files: page.relevant_files.clone(),
            related_pages: page.related_pages.clone(),
            content: fs::read_to_string(dir.join(PAGES_DIR).join(&page.file)).unwrap_or_default(),
        })
        .collect();
    Some((meta, pages))
}

pub(super) fn has_wiki_in(dir: &Path) -> bool {
    dir.is_dir()
        && fs::read_dir(dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
}

pub(super) fn begin_wiki_in(dir: &Path) -> AppResult<()> {
    let pages = dir.join(PAGES_DIR);
    if pages.exists() {
        fs::remove_dir_all(&pages)?;
    }
    for leftover in [META_FILE, "meta.json.tmp"] {
        match fs::remove_file(dir.join(leftover)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    fs::create_dir_all(&pages)?;
    Ok(())
}

#[cfg(windows)]
fn clear_readonly_recursive(root: &Path) {
    fn visit(path: &Path) {
        let Ok(meta) = fs::metadata(path) else {
            return;
        };
        if meta.is_dir() {
            if let Ok(entries) = fs::read_dir(path) {
                for entry in entries.flatten() {
                    visit(&entry.path());
                }
            }
        }
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(path, perms);
        }
    }
    visit(root);
}

#[cfg(not(windows))]
fn clear_readonly_recursive(_root: &Path) {}

pub(super) fn remove_wiki_dir(dir: &Path) -> AppResult<()> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            clear_readonly_recursive(dir);
            match fs::remove_dir_all(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}

pub(super) fn get_wiki_dir(app: AppHandle, project_path: String) -> AppResult<String> {
    Ok(wiki_dir(&app, &project_path)?
        .to_string_lossy()
        .into_owned())
}

pub(super) fn load_wiki_config(
    app: AppHandle,
    project_path: String,
) -> AppResult<WikiGenerationConfig> {
    load_wiki_config_internal(&app, &project_path)
}

pub(super) fn save_wiki_config(
    app: AppHandle,
    project_path: String,
    config: WikiGenerationConfig,
) -> AppResult<()> {
    save_config_in(&wiki_dir(&app, &project_path)?, config)
}

pub(super) fn has_wiki(app: AppHandle, project_path: String) -> AppResult<bool> {
    Ok(has_wiki_in(&wiki_dir(&app, &project_path)?))
}

pub(crate) fn begin_wiki(app: AppHandle, project_path: String) -> AppResult<()> {
    begin_wiki_in(&wiki_dir(&app, &project_path)?)
}

pub(crate) fn load_wiki_config_internal(
    app: &AppHandle,
    project_path: &str,
) -> AppResult<WikiGenerationConfig> {
    let dir = wiki_dir(app, project_path)?;
    if dir.join(CONFIG_FILE).is_file() {
        return load_config_in(&dir);
    }
    let config = legacy_wiki_config(app);
    save_config_in(&dir, config.clone())?;
    Ok(config)
}

pub(crate) fn save_wiki_page_internal(
    app: &AppHandle,
    project_path: &str,
    file_name: &str,
    content: &str,
) -> AppResult<()> {
    save_page_in(&wiki_dir(app, project_path)?, file_name, content)
}

pub(crate) fn save_wiki_meta(
    app: AppHandle,
    project_path: String,
    mut meta: WikiMeta,
    commit_kind: Option<WikiCommitKind>,
) -> AppResult<()> {
    if meta.project_path.is_empty() {
        meta.project_path = clean_str(&project_path);
    }
    let dir = wiki_dir(&app, &project_path)?;
    let kind = commit_kind.unwrap_or(WikiCommitKind::Generate);
    let message = commit_message(kind, &meta, None, head_sha(&project_path).as_deref());
    save_meta_in(&dir, meta)?;
    if let Err(e) = commit_wiki_in(&dir, &message) {
        eprintln!("[wiki] git 快照提交失败({:?}): {e}", dir);
    }
    Ok(())
}

pub(crate) fn commit_wiki(
    app: AppHandle,
    project_path: String,
    kind: WikiCommitKind,
    title: Option<String>,
) -> AppResult<()> {
    let dir = wiki_dir(&app, &project_path)?;
    let meta = fs::read_to_string(dir.join(META_FILE))
        .ok()
        .and_then(|raw| serde_json::from_str::<WikiMeta>(&raw).ok())
        .unwrap_or_default();
    let message = commit_message(
        kind,
        &meta,
        title.as_deref(),
        head_sha(&project_path).as_deref(),
    );
    commit_wiki_in(&dir, &message)
}

pub(super) fn load_wiki(app: AppHandle, project_path: String) -> AppResult<Option<WikiData>> {
    let dir = wiki_dir(&app, &project_path)?;
    let Some((meta, pages)) = load_wiki_in(&dir) else {
        return Ok(None);
    };
    let stale = match (&meta.head_sha, &head_sha(&project_path)) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };
    Ok(Some(WikiData { meta, pages, stale }))
}

pub(super) fn delete_wiki(app: AppHandle, project_path: String) -> AppResult<()> {
    remove_wiki_dir(&wiki_dir(&app, &project_path)?)
}

pub(super) fn open_wiki_dir(app: AppHandle, project_path: String) -> AppResult<()> {
    let dir = wiki_dir(&app, &project_path)?;
    fs::create_dir_all(&dir)?;
    open::open_explorer(&dir.to_string_lossy())
}
