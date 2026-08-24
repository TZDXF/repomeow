//! 项目 Wiki:AI 生成的大纲与页面落盘为 `~/.repomeow/wiki/<basename>-<hash>/` 下的
//! `config.json` + `meta.json` + `pages/NN-slug.md` 普通文件(不进 SQLite),用户可直接
//! 查看/编辑/导出。config.json 保存该项目独立的生成后端配置。
//!
//! wiki 目录本身是一个本地 git 仓库(首次提交时 `git init`):整本生成/增量更新在
//! save_wiki_meta 落盘后自动快照提交,单页重新生成走 commit_wiki;begin_wiki 只清
//! pages/ 与 meta.json 而保留 config.json 和 .git,重新生成在同一配置与历史上演进;删除(delete_wiki)
//! 不走 git,整目录直接移除(含 .git)。
//!
//! 生成流水线由 `commands::ai` 在后端编排：收集文件树与清单 → SDK/ACP 生成大纲
//! → 逐页读取相关文件并生成正文 → 原子落盘页面 → 最后写入 meta 并提交快照。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::commands::{files, git, open, walk};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::{clean_str, to_forward_slash};
use crate::APP_DATA_DIR_NAME;

const WIKI_DIR_NAME: &str = "wiki";
const CONFIG_FILE: &str = "config.json";
const META_FILE: &str = "meta.json";
const PAGES_DIR: &str = "pages";
const CONFIG_VERSION: u32 = 1;
const META_VERSION: u32 = 1;
/// wiki git 提交身份:git 管理是应用自身行为,不用用户全局配置(避免无身份时提交失败)
const WIKI_GIT_NAME: &str = "RepoMeow";
const WIKI_GIT_EMAIL: &str = "repomeow@localhost";

/// 文件树字符预算(≈25k token),超出按目录折叠
const FILE_TREE_MAX_CHARS: usize = 100_000;
/// README 注入 prompt 的字符上限
const README_MAX_CHARS: usize = 32_768;
/// 单个清单文件读取上限
const MANIFEST_MAX_BYTES: usize = 16 * 1024;
/// 单页生成时单个相关文件的字符上限
const PAGE_FILE_MAX_CHARS: usize = 65_536;
/// 单页生成时全部相关文件的总量预算,耗尽即停止读取后续文件
const PAGE_TOTAL_MAX_CHARS: usize = 240_000;

/// 目录名黑名单:任一路径段命中即排除(node_modules / 隐藏目录已由 walk 层跳过,
/// 这里主要兜住非 git 项目没有 .gitignore 时的构建产物目录)
const EXCLUDED_DIRS: &[&str] = &[
    "target",
    "dist",
    "build",
    "out",
    "coverage",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    "bin",
    "obj",
    "pods",
    "deriveddata",
];

/// 文件名黑名单(小写比较):锁文件与系统杂项对理解架构没有价值且体积极大
const EXCLUDED_FILE_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "cargo.lock",
    "composer.lock",
    "gemfile.lock",
    "poetry.lock",
    "bun.lockb",
    ".ds_store",
    "thumbs.db",
];

/// 扩展名黑名单(小写比较):二进制、媒体、字体、压缩包、sourcemap 等
const EXCLUDED_EXTS: &[&str] = &[
    "exe", "dll", "so", "dylib", "bin", "o", "a", "lib", "jar", "war", "class", "pyc", "pyo",
    "png", "jpg", "jpeg", "gif", "webp", "ico", "svg", "bmp", "avif", "mp3", "mp4", "wav", "ogg",
    "mov", "avi", "webm", "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst", "pdf",
    "woff", "woff2", "ttf", "otf", "eot", "db", "sqlite", "sqlite3", "map", "lock",
];

/// 根目录清单文件:帮助 LLM 理解技术栈与构建方式,存在才读取
const MANIFEST_NAMES: &[&str] = &[
    "package.json",
    "pnpm-workspace.yaml",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "requirements.txt",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    "tsconfig.json",
];

// ── 数据结构(IPC 边界恒为 camelCase) ─────────────────────────────────────

/// 触发 wiki git 提交的操作类型(决定提交信息措辞;序列化为 "generate"/"update"/"page")
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WikiCommitKind {
    Generate,
    Update,
    Page,
}

/// 大纲中的单个页面条目(meta.json 的 outline 元素)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiOutlinePage {
    pub id: String,
    /// 页面文件名(pages/ 下,如 `01-overview.md`)
    pub file: String,
    pub title: String,
    /// 该页覆盖内容的简述(大纲阶段产出,单页生成时注入 prompt)
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub importance: String,
    #[serde(default)]
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub related_pages: Vec<String>,
}

/// wiki 元信息;`generated_at` 与 `version` 由 save_wiki_meta 覆写,前端无需填
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiMeta {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub outline: Vec<WikiOutlinePage>,
    /// 生成后端标识("builtin" / "acp:<agentId>");旧 meta 缺省视为内置。
    /// 前端手动增量更新遇后端切换时退化为整本重生成
    #[serde(default)]
    pub generator: Option<String>,
}

/// 单个项目的 Wiki 生成配置，独立保存在该项目 Wiki 目录的 config.json。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiGenerationConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub backend: super::ai::WikiGenerationBackend,
}

const fn default_config_version() -> u32 {
    CONFIG_VERSION
}

impl Default for WikiGenerationConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            backend: super::ai::WikiGenerationBackend::Builtin,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiPageData {
    pub id: String,
    pub file: String,
    pub title: String,
    pub section: Option<String>,
    pub importance: String,
    pub relevant_files: Vec<String>,
    pub related_pages: Vec<String>,
    /// 页面 Markdown 正文;文件缺失时为空串(前端显示占位)
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiData {
    pub meta: WikiMeta,
    pub pages: Vec<WikiPageData>,
    /// 生成时的 HEAD 与当前 HEAD 不一致(代码已更新,wiki 可能过时)
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiManifest {
    pub path: String,
    pub content: String,
}

/// 结构阶段的输入:过滤后的文件树 + README + 根目录清单文件 + 当前 HEAD
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiContext {
    pub file_tree: String,
    /// 过滤后的完整文件清单(/ 分隔相对路径,不折叠),后端用于校验大纲标注的相关文件
    pub paths: Vec<String>,
    pub file_count: usize,
    pub tree_truncated: bool,
    pub readme: Option<String>,
    pub manifests: Vec<WikiManifest>,
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiFileContent {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}

// ── 目录派生 ─────────────────────────────────────────────────────────────

/// FNV-1a 64 位:自实现保证跨版本稳定(std 的 DefaultHasher 不承诺哈希值稳定)
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// wiki 文件夹名:`<basename>-<clean 路径哈希低32位 hex>`。
/// basename 取归一化路径最后一段(非法文件名字符替换为 `_`),哈希防同名碰撞
fn folder_name(project_path: &str) -> String {
    let clean = clean_str(project_path);
    let base = clean.rsplit(['\\', '/']).next().unwrap_or_default();
    let base: String = base
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let base = if base.is_empty() { "root" } else { &base };
    format!("{base}-{:08x}", fnv1a64(&clean) as u32)
}

fn wiki_dir_in(root: &Path, project_path: &str) -> PathBuf {
    root.join(folder_name(project_path))
}

fn wiki_dir(app: &AppHandle, project_path: &str) -> AppResult<PathBuf> {
    let home = app
        .path()
        .home_dir()
        .map_err(|e| AppError::coded(ErrorCode::IoError, e.to_string()))?;
    Ok(wiki_dir_in(
        &home.join(APP_DATA_DIR_NAME).join(WIKI_DIR_NAME),
        project_path,
    ))
}

/// 读取仓库当前 HEAD 的完整 sha;非 git 仓库 / 空仓库 / 读取失败均为 None
fn head_sha(project_path: &str) -> Option<String> {
    let repo = git::open_repo(project_path).ok()??;
    let sha = repo.head().ok()?.target().map(|oid| oid.to_string());
    sha
}

// ── 结构阶段:上下文收集 ──────────────────────────────────────────────────

/// 文件是否对理解项目有价值(walk 已按 gitignore 过滤,此处再排除产物/二进制/锁文件)
fn is_wiki_relevant(rel: &str) -> bool {
    let path = Path::new(rel);
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy().to_lowercase();
        if EXCLUDED_DIRS.contains(&s.as_str()) {
            return false;
        }
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_lowercase();
    if EXCLUDED_FILE_NAMES.contains(&lower.as_str()) {
        return false;
    }
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        return false;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if EXCLUDED_EXTS.contains(&ext.to_lowercase().as_str()) {
            return false;
        }
    }
    true
}

/// 渲染文件树(每行一个 `/` 分隔相对路径)。超预算时反复把「最深的、含 ≥2 个文件的
/// 父目录」折叠为 `dir/ (N files)` 摘要行(路径有序,同级兄弟必然相邻);
/// 无可折叠行仍超预算时做行级截断,保证结构信息尽量完整
fn render_file_tree(paths: &[String]) -> (String, bool) {
    if paths.join("\n").len() <= FILE_TREE_MAX_CHARS {
        return (paths.join("\n"), false);
    }
    let mut lines: Vec<String> = paths.to_vec();
    loop {
        if lines.join("\n").len() <= FILE_TREE_MAX_CHARS {
            return (lines.join("\n"), true);
        }
        // 按父目录给相邻文件行分组,取最深一组折叠
        let mut best: Option<(usize, usize, usize)> = None; // (深度, 起始下标, 组长度)
        let mut i = 0;
        while i < lines.len() {
            let Some(pos) = lines[i].rfind('/') else {
                i += 1;
                continue;
            };
            let parent = &lines[i][..pos];
            let start = i;
            while i < lines.len()
                && lines[i]
                    .rfind('/')
                    .is_some_and(|p| &lines[i][..p] == parent)
            {
                i += 1;
            }
            let len = i - start;
            if len >= 2 {
                let depth = parent.matches('/').count();
                if best.is_none_or(|(d, _, _)| depth > d) {
                    best = Some((depth, start, len));
                }
            }
        }
        let Some((_, start, len)) = best else {
            // 没有可折叠的兄弟组:保留预算内的前缀行,追加截断标记
            let mut kept = Vec::new();
            let mut used = 0;
            for line in &lines {
                if used + line.len() + 1 > FILE_TREE_MAX_CHARS {
                    break;
                }
                used += line.len() + 1;
                kept.push(line.clone());
            }
            kept.push(format!("... ({} more files)", lines.len() - kept.len()));
            return (kept.join("\n"), true);
        };
        let parent = lines[start][..lines[start].rfind('/').unwrap()].to_string();
        lines.splice(start..start + len, [format!("{parent}/ ({len} files)")]);
    }
}

/// 按 UTF-8 边界截断到 max 字节(跳过 continuation byte)
fn truncate_utf8(bytes: &[u8], max: usize) -> String {
    let mut end = bytes.len().min(max);
    while end > 0 && end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// 读取根目录清单文件(存在才读,单文件上限 MANIFEST_MAX_BYTES)
fn read_manifests(root: &Path) -> Vec<WikiManifest> {
    MANIFEST_NAMES
        .iter()
        .filter_map(|name| {
            let file = root.join(name);
            if !file.is_file() {
                return None;
            }
            let bytes = fs::read(&file).ok()?;
            Some(WikiManifest {
                path: (*name).to_string(),
                content: truncate_utf8(&bytes, MANIFEST_MAX_BYTES),
            })
        })
        .collect()
}

/// 收集结构阶段上下文:过滤后的文件树 + README(截断)+ 根目录清单文件 + 当前 HEAD
pub(crate) fn collect_wiki_context(project_path: String) -> AppResult<WikiContext> {
    files::ensure_dir(&project_path)?;
    let root = Path::new(&project_path);
    let paths: Vec<String> = walk::project_files_cached(root)
        .iter()
        .map(|p| walk::to_slash(p))
        .filter(|p| is_wiki_relevant(p))
        .collect();
    let file_count = paths.len();
    let (file_tree, tree_truncated) = render_file_tree(&paths);

    let readme = files::read_readme(&project_path)?.map(|content| {
        if content.len() > README_MAX_CHARS {
            format!(
                "{}\n\n...(truncated)",
                truncate_utf8(content.as_bytes(), README_MAX_CHARS)
            )
        } else {
            content
        }
    });

    let manifests = read_manifests(root);

    Ok(WikiContext {
        file_tree,
        paths,
        file_count,
        tree_truncated,
        readme,
        manifests,
        head_sha: head_sha(&project_path),
    })
}

/// 读取单页生成所需的相关文件全文。LLM 大纲标注的路径可能不存在或幻觉,
/// 逐个 canonicalize 校验(拒绝越界与符号链接逃逸),读不到的静默跳过;
/// 二进制文件跳过;单文件超 PAGE_FILE_MAX_CHARS 截断;总预算耗尽即停止
pub(crate) fn read_wiki_files_in(
    project_path: &str,
    rel_paths: &[String],
) -> AppResult<Vec<WikiFileContent>> {
    files::ensure_dir(&project_path)?;
    let root_canon = fs::canonicalize(&project_path)?;
    let mut out = Vec::new();
    let mut budget = PAGE_TOTAL_MAX_CHARS;
    for rel in rel_paths {
        let Ok(file) = fs::canonicalize(root_canon.join(rel)) else {
            continue;
        };
        if !file.starts_with(&root_canon) || !file.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(&file) else {
            continue;
        };
        // 二进制嗅探:前缀内出现 NUL 即跳过(与 read_file_preview 口径一致)
        if bytes[..bytes.len().min(8_000)].contains(&0) {
            continue;
        }
        let cap = PAGE_FILE_MAX_CHARS.min(budget);
        let content = truncate_utf8(&bytes, cap);
        let truncated = bytes.len() > content.len();
        budget = budget.saturating_sub(content.len());
        out.push(WikiFileContent {
            path: to_forward_slash(Path::new(&rel)),
            content,
            truncated,
        });
        if budget == 0 {
            break;
        }
    }
    Ok(out)
}

// ── 落盘与读取 ───────────────────────────────────────────────────────────

/// 页面文件名校验:防路径穿越,只允许 `NN-slug.md` 形态
fn valid_page_file(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".md")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

fn save_page_in(dir: &Path, file_name: &str, content: &str) -> AppResult<()> {
    if !valid_page_file(file_name) {
        return Err(AppError::coded(ErrorCode::InvalidPath, file_name));
    }
    let pages = dir.join(PAGES_DIR);
    fs::create_dir_all(&pages)?;
    let target = pages.join(file_name);
    // 先写 tmp 再 rename:生成中途取消不会留下半截页面文件
    let tmp = pages.join(format!("{file_name}.tmp"));
    fs::write(&tmp, content)?;
    fs::rename(&tmp, &target)?;
    Ok(())
}

fn save_meta_in(dir: &Path, mut meta: WikiMeta) -> AppResult<()> {
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

fn save_config_in(dir: &Path, mut config: WikiGenerationConfig) -> AppResult<()> {
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

fn load_config_in(dir: &Path) -> AppResult<WikiGenerationConfig> {
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

/// 从旧版全局 settings 组装一次迁移用配置。仅在项目 config.json 尚不存在时使用，
/// 写入项目目录后该项目便不再受全局值影响。
fn legacy_wiki_config(app: &AppHandle) -> WikiGenerationConfig {
    let model = crate::tray::read_setting_string(app, "wikiAgentModel").filter(|v| !v.is_empty());
    let thinking =
        crate::tray::read_setting_string(app, "wikiAgentThinking").filter(|v| !v.is_empty());
    let backend = match crate::tray::read_setting_string(app, "wikiGenBackend").as_deref() {
        None | Some("") | Some("builtin") => super::ai::WikiGenerationBackend::Builtin,
        Some("custom") => super::ai::WikiGenerationBackend::Agent {
            agent_id: None,
            custom_command: crate::tray::read_setting_string(app, "wikiAgentCustomCommand")
                .filter(|v| !v.is_empty()),
            model,
            thinking,
        },
        Some(agent_id) => super::ai::WikiGenerationBackend::Agent {
            agent_id: Some(agent_id.to_string()),
            custom_command: None,
            model,
            thinking,
        },
    };
    WikiGenerationConfig {
        version: CONFIG_VERSION,
        backend,
    }
}

fn load_wiki_in(dir: &Path) -> Option<(WikiMeta, Vec<WikiPageData>)> {
    let raw = fs::read_to_string(dir.join(META_FILE)).ok()?;
    let meta: WikiMeta = serde_json::from_str(&raw).ok()?;
    // meta.json 最后写入;status 未完结说明上次生成被中断,整本视为无效
    if meta.status != "completed" {
        return None;
    }
    let pages = meta
        .outline
        .iter()
        .map(|p| WikiPageData {
            id: p.id.clone(),
            file: p.file.clone(),
            title: p.title.clone(),
            section: p.section.clone(),
            importance: p.importance.clone(),
            relevant_files: p.relevant_files.clone(),
            related_pages: p.related_pages.clone(),
            content: fs::read_to_string(dir.join(PAGES_DIR).join(&p.file)).unwrap_or_default(),
        })
        .collect();
    Some((meta, pages))
}

/// 项目是否已有 wiki 数据目录(目录存在且非空;未完结/损坏的 meta 也算,
/// 供删除项目时询问是否一并清理)
fn has_wiki_in(dir: &Path) -> bool {
    dir.is_dir()
        && fs::read_dir(dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
}

/// 项目的 wiki 目录路径(不创建),供前端展示
#[tauri::command]
pub fn get_wiki_dir(app: AppHandle, project_path: String) -> AppResult<String> {
    Ok(wiki_dir(&app, &project_path)?
        .to_string_lossy()
        .into_owned())
}

/// 读取项目独立的 Wiki 生成配置；尚未配置时返回内置 API 默认值。
#[tauri::command]
pub fn load_wiki_config(app: AppHandle, project_path: String) -> AppResult<WikiGenerationConfig> {
    load_wiki_config_internal(&app, &project_path)
}

/// 保存项目独立的 Wiki 生成配置到其 Wiki 目录。
#[tauri::command]
pub fn save_wiki_config(
    app: AppHandle,
    project_path: String,
    config: WikiGenerationConfig,
) -> AppResult<()> {
    save_config_in(&wiki_dir(&app, &project_path)?, config)
}

/// 项目是否已有 wiki 数据(供删除项目时联动询问清理)
#[tauri::command]
pub fn has_wiki(app: AppHandle, project_path: String) -> AppResult<bool> {
    Ok(has_wiki_in(&wiki_dir(&app, &project_path)?))
}

/// 开始一次全新生成:清空 pages/ 与 meta.json(旧 wiki 随即失效,避免中断后读到
/// 新旧混杂)。保留 config.json 与 .git——项目生成配置及历史不受重新生成影响
fn begin_wiki_in(dir: &Path) -> AppResult<()> {
    let pages = dir.join(PAGES_DIR);
    if pages.exists() {
        fs::remove_dir_all(&pages)?;
    }
    // meta 与残留 tmp 一并清掉(pages/ 下的 tmp 已随目录删除)
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

// ── git 快照管理 ──────────────────────────────────────────────────────────

/// 组提交信息:固定中文,与仓库提交信息约定一致,不随应用语言切换;
/// 统一附「当前代码 HEAD 前 7 位」便于对照 wiki 版本对应的代码版本(非 git 项目省略)
fn commit_message(
    kind: WikiCommitKind,
    meta: &WikiMeta,
    title: Option<&str>,
    head: Option<&str>,
) -> String {
    let short = head.filter(|s| s.len() >= 7).map(|s| &s[..7]);
    match kind {
        WikiCommitKind::Generate => match short {
            Some(s) => format!("生成 wiki(共 {} 页,代码 {s})", meta.outline.len()),
            None => format!("生成 wiki(共 {} 页)", meta.outline.len()),
        },
        WikiCommitKind::Update => match short {
            Some(s) => format!("增量更新 wiki(代码 {s})"),
            None => "增量更新 wiki".into(),
        },
        WikiCommitKind::Page => match (title, short) {
            (Some(t), Some(s)) => format!("重新生成页面:{t}(代码 {s})"),
            (Some(t), None) => format!("重新生成页面:{t}"),
            (None, Some(s)) => format!("重新生成页面(代码 {s})"),
            (None, None) => "重新生成页面".into(),
        },
    }
}

/// 在 wiki 目录做一次快照提交:无 .git 时先 `git init`,工作区无变更则跳过(幂等)。
/// 提交固定 RepoMeow 身份并关闭 GPG 签名、跳过钩子,避免用户全局 git 配置
/// (无身份/gpg 私钥)导致提交挂起或失败
fn commit_wiki_in(dir: &Path, message: &str) -> AppResult<()> {
    let dir_str = dir.to_string_lossy().into_owned();
    if !dir.join(".git").exists() {
        git::run_git(&dir_str, &["init"])?;
        // 本地固化身份与签名开关:用户在该目录手动操作 git 时行为一致
        for (key, value) in [
            ("user.name", WIKI_GIT_NAME),
            ("user.email", WIKI_GIT_EMAIL),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git::run_git(&dir_str, &["config", key, value])?;
        }
    }
    // status --porcelain 空输出 = 无变更,直接跳过
    let status = git::git_command(&dir_str)
        .args(["status", "--porcelain"])
        .output()
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    if !status.status.success() {
        return Err(AppError::coded(
            ErrorCode::GitCommandFailed,
            format!(
                "git status: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            ),
        ));
    }
    if status.stdout.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(());
    }
    git::run_git(&dir_str, &["add", "-A"])?;
    let args = [
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--no-verify",
        "-m",
        message,
    ];
    git::run_git(&dir_str, &args)?;
    Ok(())
}

/// 清除目录树下所有文件的只读位(Windows:git 对象文件只读,直接删会「拒绝访问」)
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

/// 删除 wiki 目录(含 .git):不走 git,直接移除;只读位导致失败时清权限重试
fn remove_wiki_dir(dir: &Path) -> AppResult<()> {
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

/// 开始一次全新生成:清空旧 pages/ 与 meta.json,保留 .git 历史
pub(crate) fn begin_wiki(app: AppHandle, project_path: String) -> AppResult<()> {
    begin_wiki_in(&wiki_dir(&app, &project_path)?)
}

/// 后端生成/更新任务读取项目独立配置的统一入口。
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

/// 写入单个页面(tmp + rename);file_name 必须匹配 `NN-slug.md`
pub(crate) fn save_wiki_page_internal(
    app: &AppHandle,
    project_path: &str,
    file_name: &str,
    content: &str,
) -> AppResult<()> {
    save_page_in(&wiki_dir(app, project_path)?, file_name, content)
}

/// 写入 meta.json(最后调用;version 与 generated_at 由后端覆写),随后自动做一次
/// git 快照提交。meta 落盘成功即整本有效,提交失败仅记日志不阻断(下次操作会补提交)
pub(crate) fn save_wiki_meta(
    app: AppHandle,
    project_path: String,
    meta: WikiMeta,
    commit_kind: Option<WikiCommitKind>,
) -> AppResult<()> {
    let mut meta = meta;
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

/// 手动触发一次 git 快照提交:单页重新生成等不经 save_wiki_meta 的场景。
/// kind=Page 需要 title;generate/update 读盘上现有 meta 组提交信息
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

/// 读取整个 wiki;meta 缺失/损坏/未完结返回 None;附带与当前 HEAD 比对的 stale 标记
#[tauri::command]
pub fn load_wiki(app: AppHandle, project_path: String) -> AppResult<Option<WikiData>> {
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

/// 删除项目的整个 wiki 目录(幂等;含 .git,不走 git 操作)
#[tauri::command]
pub fn delete_wiki(app: AppHandle, project_path: String) -> AppResult<()> {
    remove_wiki_dir(&wiki_dir(&app, &project_path)?)
}

/// 在系统文件管理器中打开项目的 wiki 目录
#[tauri::command]
pub fn open_wiki_dir(app: AppHandle, project_path: String) -> AppResult<()> {
    let dir = wiki_dir(&app, &project_path)?;
    fs::create_dir_all(&dir)?;
    open::open_explorer(&dir.to_string_lossy())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiChangedFiles {
    pub files: Vec<String>,
    /// 当前 HEAD(增量更新成功后回写 meta)
    pub head_sha: Option<String>,
}

/// 增量更新用:列出 from_sha..HEAD 之间变更的文件(/ 分隔相对路径)。
/// 非 git 仓库返回空表(调用方退化为整本重生成);from_sha 无法解析时报错
/// (仓库历史被改写,调用方同样退化为整本重生成)
pub(crate) fn wiki_changed_files(
    project_path: String,
    from_sha: String,
) -> AppResult<WikiChangedFiles> {
    let Some(repo) = git::open_repo(&project_path)? else {
        return Ok(WikiChangedFiles {
            files: Vec::new(),
            head_sha: None,
        });
    };
    let oid = git2::Oid::from_str(&from_sha)
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let from = repo
        .find_commit(oid)
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let Ok(head) = repo.head().and_then(|h| h.peel_to_commit()) else {
        return Ok(WikiChangedFiles {
            files: Vec::new(),
            head_sha: None,
        });
    };
    let from_tree = from
        .tree()
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let head_tree = head
        .tree()
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let diff = repo
        .diff_tree_to_tree(Some(&from_tree), Some(&head_tree), None)
        .map_err(|e| AppError::coded(ErrorCode::GitCommandFailed, e.to_string()))?;
    let mut files: Vec<String> = diff
        .deltas()
        // 删除的文件用 old_file,其余用 new_file(重命名取新路径)
        .filter_map(|d| {
            let p = if d.status() == git2::Delta::Deleted {
                d.old_file().path()
            } else {
                d.new_file().path()
            };
            p.map(to_forward_slash)
        })
        .collect();
    files.sort();
    files.dedup();
    Ok(WikiChangedFiles {
        files,
        head_sha: Some(head.id().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-wiki-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folder_name_distinguishes_same_basename() {
        let a = folder_name("D:/code/web");
        let b = folder_name("E:/other/web");
        assert!(a.starts_with("web-") && b.starts_with("web-"));
        assert_ne!(a, b, "同名不同路径的项目必须落到不同 wiki 目录");
        // 尾随分隔符归一后为同一目录
        assert_eq!(folder_name("D:/code/web/"), folder_name("D:/code/web"));
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        save_page_in(&dir, "01-overview.md", "# 概览").unwrap();
        let meta = WikiMeta {
            status: "completed".into(),
            outline: vec![WikiOutlinePage {
                id: "overview".into(),
                file: "01-overview.md".into(),
                title: "概览".into(),
                importance: "high".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        save_meta_in(&dir, meta).unwrap();

        let (meta, pages) = load_wiki_in(&dir).unwrap();
        assert_eq!(meta.version, META_VERSION);
        assert!(!meta.generated_at.is_empty(), "generated_at 由后端覆写");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].content, "# 概览");
        // tmp 文件不残留
        assert!(!dir.join(PAGES_DIR).join("01-overview.md.tmp").exists());
        assert!(!dir.join("meta.json.tmp").exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generation_config_roundtrip_and_default() {
        let dir = temp_dir("config-roundtrip");
        assert!(matches!(
            load_config_in(&dir).unwrap().backend,
            crate::commands::ai::WikiGenerationBackend::Builtin
        ));

        save_config_in(
            &dir,
            WikiGenerationConfig {
                version: 99,
                backend: crate::commands::ai::WikiGenerationBackend::Agent {
                    agent_id: Some("codex".into()),
                    custom_command: None,
                    model: Some("gpt-5".into()),
                    thinking: Some("high".into()),
                },
            },
        )
        .unwrap();
        let loaded = load_config_in(&dir).unwrap();
        assert_eq!(loaded.version, CONFIG_VERSION, "保存时应覆写配置版本");
        match loaded.backend {
            crate::commands::ai::WikiGenerationBackend::Agent {
                agent_id,
                model,
                thinking,
                ..
            } => {
                assert_eq!(agent_id.as_deref(), Some("codex"));
                assert_eq!(model.as_deref(), Some("gpt-5"));
                assert_eq!(thinking.as_deref(), Some("high"));
            }
            crate::commands::ai::WikiGenerationBackend::Builtin => panic!("应读回 agent 配置"),
        }
        assert!(!dir.join("config.json.tmp").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_returns_none_when_incomplete_or_missing() {
        let dir = temp_dir("incomplete");
        assert!(load_wiki_in(&dir).is_none(), "无 meta.json 返回 None");
        save_meta_in(
            &dir,
            WikiMeta {
                status: "generating".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(load_wiki_in(&dir).is_none(), "未完结的 meta 视为无效");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_traversal_page_name() {
        let dir = temp_dir("traversal");
        assert!(save_page_in(&dir, "../evil.md", "x").is_err());
        assert!(save_page_in(&dir, "a/b.md", "x").is_err());
        assert!(save_page_in(&dir, "01-ok.md", "x").is_ok());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_tree_folds_when_over_budget() {
        // 构造超预算的文件树:多个深层目录,每目录多个文件
        let mut paths = Vec::new();
        for d in 0..50 {
            for f in 0..50 {
                paths.push(format!(
                    "src/module-{d:03}/sub/file-{f:03}-with-a-long-name.rs"
                ));
            }
        }
        let (tree, truncated) = render_file_tree(&paths);
        assert!(truncated);
        assert!(tree.len() <= FILE_TREE_MAX_CHARS + 64);
        assert!(tree.contains("files)"), "应以目录折叠摘要为主: {tree}");
    }

    #[test]
    fn wiki_relevance_filters() {
        assert!(is_wiki_relevant("src/main.rs"));
        assert!(is_wiki_relevant("docs/guide.md"));
        assert!(!is_wiki_relevant("target/debug/app.exe"));
        assert!(!is_wiki_relevant("dist/bundle.js"));
        assert!(!is_wiki_relevant("pnpm-lock.yaml"));
        assert!(!is_wiki_relevant("assets/logo.png"));
        assert!(!is_wiki_relevant("src/app.min.js"));
        assert!(!is_wiki_relevant("data/app.sqlite"));
    }

    #[test]
    fn manifests_only_include_existing() {
        let dir = temp_dir("manifest");
        fs::write(dir.join("package.json"), "{}").unwrap();
        let manifests = read_manifests(&dir);
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].path, "package.json");
        assert_eq!(manifests[0].content, "{}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn begin_wiki_keeps_git_dir() {
        let dir = temp_dir("begin-keep-git");
        fs::create_dir_all(dir.join(PAGES_DIR)).unwrap();
        fs::write(dir.join(PAGES_DIR).join("01-old.md"), "old").unwrap();
        fs::write(dir.join(META_FILE), "{}").unwrap();
        fs::write(
            dir.join(CONFIG_FILE),
            "{\"version\":1,\"backend\":{\"kind\":\"builtin\"}}",
        )
        .unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();

        begin_wiki_in(&dir).unwrap();
        assert!(dir.join(".git").is_dir(), "重新生成必须保留 .git 历史");
        assert!(!dir.join(META_FILE).exists());
        assert!(!dir.join(PAGES_DIR).join("01-old.md").exists());
        assert!(dir.join(PAGES_DIR).is_dir(), "pages/ 应重建为空目录");
        assert!(dir.join(CONFIG_FILE).is_file(), "重新生成必须保留项目配置");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn changed_files_since() {
        let dir = temp_dir("changed-files");
        let repo = git2::Repository::init(&dir).unwrap();
        let sig = git2::Signature::now("t", "t@localhost").unwrap();
        let mut oids = Vec::new();
        for i in 0..3 {
            let blob = repo.blob(format!("v{i}").as_bytes()).unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("f.txt", blob, 0o100644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            let parents: Vec<git2::Commit> = oids
                .last()
                .map(|oid| repo.find_commit(*oid).unwrap())
                .into_iter()
                .collect();
            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            oids.push(
                repo.commit(
                    Some("HEAD"),
                    &sig,
                    &sig,
                    &format!("c{i}"),
                    &tree,
                    &parent_refs,
                )
                .unwrap(),
            );
        }
        let path = dir.to_string_lossy().to_string();

        let r = wiki_changed_files(path.clone(), oids[0].to_string()).unwrap();
        assert_eq!(r.files, vec!["f.txt".to_string()]);

        let r = wiki_changed_files(path.clone(), oids[2].to_string()).unwrap();
        assert!(r.files.is_empty(), "from == HEAD 时不应有变更文件");
        assert_eq!(r.head_sha.as_deref(), Some(oids[2].to_string().as_str()));

        fs::remove_dir_all(&dir).ok();
    }

    /// 环境没有 git CLI 时跳过(commit_wiki_in 依赖系统 git)
    fn git_available() -> bool {
        git::git_command(".")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn completed_meta(file: &str) -> WikiMeta {
        WikiMeta {
            status: "completed".into(),
            head_sha: Some("0123456789abcdef".into()),
            outline: vec![WikiOutlinePage {
                id: "overview".into(),
                file: file.into(),
                title: "概览".into(),
                importance: "high".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn commit_wiki_snapshots_and_skips_when_clean() {
        if !git_available() {
            return;
        }
        let dir = temp_dir("git-commit");
        save_page_in(&dir, "01-overview.md", "# v1").unwrap();
        save_meta_in(&dir, completed_meta("01-overview.md")).unwrap();

        commit_wiki_in(&dir, "生成 wiki(共 1 页)").unwrap();
        // 无变更时幂等跳过,不产生新提交
        commit_wiki_in(&dir, "重复提交").unwrap();
        let repo = git2::Repository::open(&dir).unwrap();
        let count = || {
            let mut walk = repo.revwalk().unwrap();
            walk.push_head().unwrap();
            walk.count()
        };
        assert_eq!(count(), 1, "无变更不应产生新提交");

        // 页面变化后提交进入历史,身份固定为 RepoMeow
        save_page_in(&dir, "01-overview.md", "# v2").unwrap();
        commit_wiki_in(&dir, "重新生成页面:概览").unwrap();
        assert_eq!(count(), 2);
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.author().name(), Some(WIKI_GIT_NAME));
        assert_eq!(head.message().unwrap().trim_end(), "重新生成页面:概览");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_messages_by_kind() {
        let meta = completed_meta("01-overview.md");
        let sha = "0123456789abcdef";
        // 三类操作都附当前代码 HEAD 短 sha
        assert_eq!(
            commit_message(WikiCommitKind::Generate, &meta, None, Some(sha)),
            "生成 wiki(共 1 页,代码 0123456)"
        );
        assert_eq!(
            commit_message(WikiCommitKind::Update, &meta, None, Some(sha)),
            "增量更新 wiki(代码 0123456)"
        );
        assert_eq!(
            commit_message(WikiCommitKind::Page, &meta, Some("概览"), Some(sha)),
            "重新生成页面:概览(代码 0123456)"
        );
        // 非 git 项目(无 HEAD)省略代码版本段
        assert_eq!(
            commit_message(WikiCommitKind::Generate, &meta, None, None),
            "生成 wiki(共 1 页)"
        );
        assert_eq!(
            commit_message(WikiCommitKind::Update, &meta, None, None),
            "增量更新 wiki"
        );
        assert_eq!(
            commit_message(WikiCommitKind::Page, &meta, Some("概览"), None),
            "重新生成页面:概览"
        );
    }

    #[test]
    fn remove_wiki_dir_tolerates_readonly_files() {
        let dir = temp_dir("remove-readonly");
        let git_objects = dir.join(".git").join("objects");
        fs::create_dir_all(&git_objects).unwrap();
        let object = git_objects.join("abc123");
        fs::write(&object, b"pack").unwrap();
        let mut perms = fs::metadata(&object).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&object, perms).unwrap();

        remove_wiki_dir(&dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn has_wiki_requires_nonempty_dir() {
        let dir = temp_dir("has-wiki");
        assert!(!has_wiki_in(&dir), "目录不存在不算有 wiki");
        fs::create_dir_all(&dir).unwrap();
        assert!(!has_wiki_in(&dir), "空目录不算有 wiki");
        fs::write(dir.join(META_FILE), "{}").unwrap();
        assert!(has_wiki_in(&dir));
        fs::remove_dir_all(&dir).ok();
    }
}
