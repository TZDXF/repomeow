use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::commands::open;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::{clean_str, to_forward_slash_str};
use crate::time_util::now_ts_nanos;

use super::paths::{head_sha, wiki_dir};
use super::snapshot::{commit_message, commit_wiki_in};
use super::types::{
    WikiCommitKind, WikiData, WikiGenerationConfig, WikiMeta, WikiOutlinePage, WikiPageData,
    CONFIG_VERSION,
};

pub(super) const CONFIG_FILE: &str = "config.json";
pub(super) const META_FILE: &str = "meta.json";
pub(super) const PAGES_DIR: &str = "pages";
pub(super) const META_VERSION: u32 = 1;
/// 页面暂存文件统一前缀。刻意含 `_` 使其**不通过** `valid_page_file`,
/// 保证暂存文件永远不会被当作正式页;git 快照前按此前缀批量清理遗留。
pub(super) const STAGING_PREFIX: &str = ".staging_";
/// 暂存页内容大小上限(字节)
pub(super) const MAX_STAGED_PAGE_BYTES: usize = 2 * 1024 * 1024;

const SOURCES_OPEN: &str = "<!--";
const SOURCES_KEYWORD: &str = "sources";
const SOURCES_CLOSE: &str = "-->";

pub(super) fn valid_page_file(name: &str) -> bool {
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
        None | Some("") | Some("builtin") => crate::commands::ai::WikiGenerationBackend::Builtin {
            model: None,
            thinking: None,
            concurrency: None,
        },
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

// ── 页面暂存事务(Agent 直接写入)───────────────────────────────────────────
// Agent 后端不再由前端把回答回传落盘,而是拿到 Wiki 目录内的暂存文件路径自行写入;
// 完成后校验内容并原子提升为正式页,失败/取消时清理,崩溃遗留按前缀批量清走。

/// run_id 只保留文件名安全字符,避免引入子目录或路径穿越
fn staging_run_tag(run_id: &str) -> String {
    let mut tag: String = run_id
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    tag.truncate(64);
    if tag.is_empty() {
        tag.push_str("run");
    }
    tag
}

/// 暂存文件路径:pages/.staging_{run tag}_{正式页名}。
/// 正式页名必须通过 valid_page_file;暂存名因含 `_` 不通过它(见 STAGING_PREFIX)
fn staging_path_in(dir: &Path, run_id: &str, file_name: &str) -> AppResult<PathBuf> {
    if !valid_page_file(file_name) {
        return Err(AppError::coded(ErrorCode::InvalidPath, file_name));
    }
    Ok(dir
        .join(PAGES_DIR)
        .join(format!("{STAGING_PREFIX}{}_{file_name}", staging_run_tag(run_id))))
}

/// 为 (project, run id, page) 创建唯一暂存文件并返回其绝对路径。
/// 更新场景(正式页已存在)从正式页复制初始内容(旧版保底),首次生成为空文件。
pub(crate) fn begin_wiki_page_staging_in(
    dir: &Path,
    run_id: &str,
    page: &WikiOutlinePage,
) -> AppResult<String> {
    let staging = staging_path_in(dir, run_id, &page.file)?;
    fs::create_dir_all(staging.parent().expect("staging has parent"))?;
    let official = dir.join(PAGES_DIR).join(&page.file);
    if official.is_file() {
        fs::copy(&official, &staging)?;
    } else {
        fs::write(&staging, "")?;
    }
    Ok(staging.to_string_lossy().into_owned())
}

/// 读取暂存内容用于预览;文件尚不存在时返回空串
pub(crate) fn read_wiki_page_staging_in(
    dir: &Path,
    run_id: &str,
    file_name: &str,
) -> AppResult<String> {
    let staging = staging_path_in(dir, run_id, file_name)?;
    match fs::read_to_string(&staging) {
        Ok(content) => Ok(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

/// 取消/失败:删除暂存文件(缺失幂等)。内容校验失败时**不会**走到这里,
/// 暂存文件保留,由调用方决定重试还是取消。
pub(crate) fn cancel_wiki_page_staging_in(
    dir: &Path,
    run_id: &str,
    file_name: &str,
) -> AppResult<()> {
    let staging = staging_path_in(dir, run_id, file_name)?;
    match fs::remove_file(&staging) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// 按固定前缀批量清理 pages/ 下的暂存文件与提升备份(git 快照前调用),返回清理数
pub(super) fn cleanup_wiki_page_staging_in(dir: &Path) -> AppResult<usize> {
    let pages = dir.join(PAGES_DIR);
    let entries = match fs::read_dir(&pages) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(STAGING_PREFIX) && entry.path().is_file() {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn staged_invalid(page: &WikiOutlinePage, reason: &str) -> AppError {
    AppError::coded(
        ErrorCode::AiResponseParseFailed,
        format!("{}: {reason}", page.file),
    )
}

/// 归一化 sources 条目路径:与前端 normalizeFilePath 及大纲校验的 normalize_path 一致
/// (统一 / 分隔、去 `./` 与 `/` 前缀),统一走 path_util,不做 ad-hoc replace
fn normalize_source_path(raw: &str) -> String {
    let mut path = to_forward_slash_str(raw);
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_string();
    }
    if let Some(stripped) = path.strip_prefix('/') {
        path = stripped.to_string();
    }
    path
}

/// 解析 `:start` / `:start-end` 行区间(1-based 闭区间,与前端 parseWikiSources 一致)。
/// Ok(None) = 冒号后不是行号(整段视为路径);Err = 形似行号但非法
fn parse_line_range(spec: &str) -> Result<Option<(u64, u64)>, String> {
    let trimmed = spec.trim();
    let looks_numeric = !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-')
        && trimmed.chars().any(|c| c.is_ascii_digit());
    if !looks_numeric {
        return Ok(None);
    }
    let invalid = || "行区间非法(须 1-based 且 start <= end)".to_string();
    let (start_raw, end_raw) = match trimmed.split_once('-') {
        Some((a, b)) => (a.trim(), Some(b.trim())),
        None => (trimmed, None),
    };
    let start: u64 = start_raw.parse().map_err(|_| invalid())?;
    let end: u64 = match end_raw {
        Some(e) => e.parse().map_err(|_| invalid())?,
        None => start,
    };
    if start < 1 || end < start {
        return Err(invalid());
    }
    Ok(Some((start, end)))
}

/// 解析单条来源:返回 Ok(None) 表示空白行(跳过);路径归一为 / 分隔
fn parse_source_entry(line: &str) -> Result<Option<(String, Option<(u64, u64)>)>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // 从右找 `:` 后接数字区间;否则整段视为路径(与前端惰性匹配兜底一致)
    let (raw_path, range) = match trimmed.rfind(':') {
        Some(pos) => match parse_line_range(&trimmed[pos + 1..])? {
            Some(range) => (&trimmed[..pos], Some(range)),
            None => (trimmed, None),
        },
        None => (trimmed, None),
    };
    let path = normalize_source_path(raw_path);
    if path.is_empty() || path == "." {
        return Err("来源条目路径为空".into());
    }
    Ok(Some((path, range)))
}

/// 提取末尾闭合的 sources 注释块条目区(取最后一个 `-->`,其后必须无非空内容)。
/// 与前端 SOURCES_BLOCK_RE 的「最后一个块」语义一致
fn sources_entries(content: &str) -> Result<&str, String> {
    let close = content
        .rfind(SOURCES_CLOSE)
        .ok_or_else(|| "缺少闭合的 sources 注释块".to_string())?;
    if !content[close + SOURCES_CLOSE.len()..].trim().is_empty() {
        return Err("sources 块之后存在非空内容".into());
    }
    let open = content[..close]
        .rfind(SOURCES_OPEN)
        .ok_or_else(|| "sources 起始注释缺失".to_string())?;
    let head = &content[open + SOURCES_OPEN.len()..close];
    let keyword = &head[head.len() - head.trim_start().len()..];
    let keyword = keyword
        .strip_prefix(SOURCES_KEYWORD)
        .ok_or_else(|| "末尾注释块不是 sources".to_string())?;
    // `sources` 之后必须直接换行(允许空白与 \r\n),条目从换行后开始
    let newline = keyword
        .find('\n')
        .ok_or_else(|| "sources 关键字后缺少换行".to_string())?;
    if !keyword[..newline].trim().is_empty() {
        return Err("sources 关键字后必须直接换行".into());
    }
    Ok(&keyword[newline + 1..])
}

/// 来源条目路径校验:优先严格命中 relevantFiles(含 bare filename 按 basename 补全,
/// 与前端 parseWikiSources 一致);未命中时回退「项目内真实文件」,`..`/绝对路径/盘符一律拒绝
fn source_path_allowed(path: &str, page: &WikiOutlinePage, project_root: &Path) -> bool {
    let known: HashSet<String> = page
        .relevant_files
        .iter()
        .map(|f| normalize_source_path(f))
        .collect();
    if known.contains(path) {
        return true;
    }
    if !path.contains('/') && known.iter().any(|f| f.rsplit('/').next() == Some(path)) {
        return true;
    }
    if path.split('/').any(|seg| seg == "..")
        || path.contains(':')
        || Path::new(path).is_absolute()
    {
        return false;
    }
    let Ok(root) = project_root.canonicalize() else {
        return false;
    };
    let Ok(candidate) = project_root.join(path).canonicalize() else {
        return false;
    };
    candidate.starts_with(root) && candidate.is_file()
}

/// 校验暂存页内容:非空且不超过大小上限、首个非空行精确 `# {page.title}`、
/// 末尾有闭合 sources 块且后无非空内容、3-10 条且路径合法
pub(super) fn validate_staged_page(
    content: &str,
    page: &WikiOutlinePage,
    project_path: &str,
) -> AppResult<()> {
    if content.trim().is_empty() {
        return Err(staged_invalid(page, "内容为空"));
    }
    if content.len() > MAX_STAGED_PAGE_BYTES {
        return Err(staged_invalid(page, "内容超出大小上限"));
    }
    let expected = format!("# {}", page.title.trim());
    let heading = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    if heading.trim() != expected {
        return Err(staged_invalid(page, &format!("首个非空行必须精确为 `{expected}`")));
    }
    let entries = sources_entries(content).map_err(|reason| staged_invalid(page, &reason))?;
    let project_root = Path::new(project_path);
    let mut count = 0usize;
    let mut seen = HashSet::new();
    for line in entries.lines() {
        let Some((path, _range)) =
            parse_source_entry(line).map_err(|reason| staged_invalid(page, &reason))?
        else {
            continue;
        };
        if !source_path_allowed(&path, page, project_root) {
            return Err(staged_invalid(
                page,
                &format!("来源 `{path}` 不在该页 relevantFiles 且不是项目内真实文件"),
            ));
        }
        if !seen.insert(path.clone()) {
            return Err(staged_invalid(page, &format!("来源 `{path}` 重复")));
        }
        count += 1;
    }
    if !(3..=10).contains(&count) {
        return Err(staged_invalid(
            page,
            &format!("sources 条目必须 3-10 条,实际 {count} 条"),
        ));
    }
    Ok(())
}

/// 校验通过后的原子提升:目标已存在时先挪到同前缀备份名(Windows rename 不保证
/// 覆盖语义,且中途失败须可回滚),提升失败恢复备份,成功后删除备份
pub(super) fn promote_validated(dir: &Path, file_name: &str, staging: &Path) -> AppResult<()> {
    if !valid_page_file(file_name) {
        return Err(AppError::coded(ErrorCode::InvalidPath, file_name));
    }
    let pages = dir.join(PAGES_DIR);
    let target = pages.join(file_name);
    let backup = pages.join(format!(
        "{STAGING_PREFIX}bak_{}_{}",
        now_ts_nanos(),
        file_name
    ));
    let had_target = target.is_file();
    if had_target {
        fs::rename(&target, &backup)?;
    }
    match fs::rename(staging, &target) {
        Ok(()) => {
            if had_target {
                let _ = fs::remove_file(&backup);
            }
            Ok(())
        }
        Err(error) => {
            if had_target {
                let _ = fs::rename(&backup, &target);
            }
            Err(error.into())
        }
    }
}

/// 读取磁盘上的暂存内容 → 校验 → 原子提升为正式页。
/// 校验失败时暂存文件保留(调用方决定重试或取消)
pub(crate) fn promote_wiki_page_staging_in(
    dir: &Path,
    project_path: &str,
    run_id: &str,
    page: &WikiOutlinePage,
) -> AppResult<()> {
    let staging = staging_path_in(dir, run_id, &page.file)?;
    let content = fs::read_to_string(&staging)?;
    validate_staged_page(&content, page, project_path)?;
    promote_validated(dir, &page.file, &staging)
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
    cleanup_wiki_page_staging_in(&dir)?;
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
    cleanup_wiki_page_staging_in(&dir)?;
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
