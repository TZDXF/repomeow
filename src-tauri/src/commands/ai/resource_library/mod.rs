//! 全局资源库 Tauri 命令层。
//!
//! 数据布局:`~/.repomeow/resource-library/{library.json, skills.json, mcp.json, skills/<directory>/SKILL.md}`,
//! 目录本身为 git 仓库(由后端自动初始化)。
//!
//! - 全部 CRUD/正文保存**自动 git init + 快照提交**;
//! - 配置了 remote 后自动触发后台同步(全局串行),同步结果写回
//!   `library.json.lastSync` 并经 `resource-library://sync-completed` 事件推送,
//!   **本地保存不因网络失败整体报错**;
//! - 加密为可选:Argon2id + XChaCha20Poly1305,口令仅内存(重启后需 unlock),
//!   启用/关闭会重建 git 历史清除明文提交,有 remote 时 force-with-lease 强推;
//! - 互斥:进程内 `Mutex`(应用 single-instance 单进程),git 网络操作另经
//!   异步 `SYNC_LOCK` 串行,避免并发 fetch/push 争抢 refs。

mod crypto;
mod errors;
mod frontmatter;
mod git;
mod import;
mod marketplace;
mod models;
mod ops;
mod scan;
mod scan_agent;
pub(super) mod store;

#[cfg(test)]
mod tests;

pub use errors::{RlError, RlResult};
pub use models::*;

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use tauri::{AppHandle, Emitter, State};

use super::run::RegisteredRun;
use crate::ai::prompts::{fixed_system_prompt, DEFAULT_SKILL_SCAN_PROMPT};
use crate::db::Db;
use crate::error::{AppError, ErrorCode};

use errors::codes;
use models::{
    MarketplaceUpdateStatus, SkillFileContent, SkillScanReport, SkillTokenReport, SyncOutcome,
    SyncRecord,
};
use store::{lock_op, Library};

/// 后台自动同步完成事件(负载为 SyncOutcome)
pub const SYNC_EVENT: &str = "resource-library://sync-completed";

/// 同步互斥:后台自动同步与显式同步命令全局串行
static SYNC_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn spawn_err(e: tokio::task::JoinError) -> RlError {
    RlError::App(AppError::coded(ErrorCode::GitTaskFailed, e.to_string()))
}

/// 供项目 AI 资产域「一键导入」复用:在库内创建技能(仅 SKILL.md 正文,
/// 其余文件由调用方写入 skills/<directory>/)。
pub(crate) fn import_skill(
    lib: &Library,
    name: &str,
    description: Option<String>,
    body: String,
) -> RlResult<models::Skill> {
    ops::skill_create(lib, name, description, vec![], Some(body))
}

/// 供项目 AI 资产域「一键导入」复用:在库内创建 MCP 服务器。
pub(crate) fn import_mcp(lib: &Library, def: &McpServerInput) -> RlResult<models::McpServer> {
    ops::mcp_create(lib, def)
}

async fn blocking<T, F>(f: F) -> RlResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> RlResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(spawn_err)?
}

fn to_record(outcome: &SyncOutcome) -> SyncRecord {
    SyncRecord {
        at: crate::time_util::now_ts(),
        ok: outcome.ok,
        error_code: outcome.error_code.clone(),
        error_message: outcome.error_message.clone(),
        ahead: outcome.ahead,
        behind: outcome.behind,
        diverged: outcome.diverged,
    }
}

async fn record_and_emit(app: &AppHandle, lib: &Library, outcome: &SyncOutcome) {
    let record = to_record(outcome);
    let lib_for_write = lib.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || lib_for_write.record_sync(&record)).await {
        eprintln!("[resource-library] 记录同步结果失败: {e}");
    }
    let _ = app.emit(SYNC_EVENT, outcome);
}

/// 写操作成功后:配置了 remote 才触发后台自动同步(网络失败不外抛)
fn maybe_trigger_sync(app: &AppHandle, lib: &Library) {
    let app = app.clone();
    let lib = lib.clone();
    tauri::async_runtime::spawn(async move {
        let lib_check = lib.clone();
        let has_remote = tokio::task::spawn_blocking(move || {
            git::remote_get(&lib_check).unwrap_or(None).is_some()
        })
        .await
        .unwrap_or(false);
        if !has_remote {
            return;
        }
        let _guard = SYNC_LOCK.lock().await;
        let lib_run = lib.clone();
        let outcome = match tokio::task::spawn_blocking(move || git::sync_once_impl(&lib_run)).await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                let mut outcome = SyncOutcome::default();
                outcome.error_code = Some("io_error".to_string());
                outcome.error_message = Some(e.to_string());
                outcome
            }
        };
        record_and_emit(&app, &lib, &outcome).await;
    });
}

/// 应用启动时的非阻塞同步检查:配置了 remote 才在后台跑一次同步,
/// 结果照常记录并推送事件;失败静默(设置页可看最近一次同步状态)。
pub fn startup_sync_check(app: &AppHandle) {
    let lib = match Library::app(app) {
        Ok(lib) => lib,
        Err(_) => return,
    };
    maybe_trigger_sync(app, &lib);
}

/// 写命令骨架:进程锁内执行业务逻辑,成功后触发后台自动同步
async fn mutate<T, F>(app: &AppHandle, f: F) -> RlResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Library) -> RlResult<T> + Send + 'static,
{
    let lib = Library::app(app)?;
    let lib_work = lib.clone();
    let result = blocking(move || {
        let _guard = lock_op();
        f(&lib_work)
    })
    .await?;
    maybe_trigger_sync(app, &lib);
    Ok(result)
}

/// 写命令骨架(不触发自动同步:git 配置/加密等自行处理同步)
async fn mutate_quiet<T, F>(app: &AppHandle, f: F) -> RlResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Library) -> RlResult<T> + Send + 'static,
{
    let lib = Library::app(app)?;
    blocking(move || {
        let _guard = lock_op();
        f(&lib)
    })
    .await
}

// ── 库 / 元信息 ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn rl_library_info(app: AppHandle) -> RlResult<LibraryInfo> {
    mutate_quiet(&app, ops::library_info).await
}

#[tauri::command]
pub async fn rl_library_open_dir(app: AppHandle) -> RlResult<()> {
    mutate_quiet(&app, ops::library_open_dir).await
}

#[tauri::command]
pub fn rl_encryption_status(app: AppHandle) -> RlResult<EncryptionStatus> {
    let lib = Library::app(&app)?;
    let _guard = lock_op();
    ops::encryption_status(&lib)
}

// ── Skill 多分组 CRUD ──────────────────────────────────────────────────

#[tauri::command]
pub fn rl_skill_list(app: AppHandle) -> RlResult<SkillLibrary> {
    let lib = Library::app(&app)?;
    let _guard = lock_op();
    ops::skill_list(&lib)
}

#[tauri::command]
pub async fn rl_skill_group_create(
    app: AppHandle,
    name: String,
    description: Option<String>,
    color: Option<String>,
) -> RlResult<SkillGroup> {
    mutate(&app, move |lib| {
        ops::group_create(lib, &name, description, color)
    })
    .await
}

#[tauri::command]
pub async fn rl_skill_group_rename(
    app: AppHandle,
    id: String,
    name: String,
    description: Option<String>,
    color: Option<String>,
) -> RlResult<SkillGroup> {
    mutate(&app, move |lib| {
        ops::group_rename(lib, &id, &name, description, color)
    })
    .await
}

#[tauri::command]
pub async fn rl_skill_group_delete(app: AppHandle, id: String) -> RlResult<()> {
    mutate(&app, move |lib| ops::group_delete(lib, &id)).await
}

#[tauri::command]
pub async fn rl_skill_group_reorder(app: AppHandle, ids: Vec<String>) -> RlResult<()> {
    mutate(&app, move |lib| ops::group_reorder(lib, &ids)).await
}

/// 以分组维度整体设定成员技能(差量增删多对多关联)
#[tauri::command]
pub async fn rl_skill_group_set_skills(
    app: AppHandle,
    group_id: String,
    skill_ids: Vec<String>,
) -> RlResult<()> {
    mutate(&app, move |lib| {
        ops::group_set_skills(lib, &group_id, &skill_ids)
    })
    .await
}

#[tauri::command]
pub async fn rl_skill_open_dir(app: AppHandle, id: String) -> RlResult<()> {
    mutate_quiet(&app, move |lib| ops::skill_open_dir(lib, &id)).await
}

#[tauri::command]
pub async fn rl_skill_create(
    app: AppHandle,
    name: String,
    description: Option<String>,
    group_ids: Vec<String>,
    body: Option<String>,
) -> RlResult<Skill> {
    mutate(&app, move |lib| {
        ops::skill_create(lib, &name, description, group_ids, body)
    })
    .await
}

#[tauri::command]
pub async fn rl_skill_update(
    app: AppHandle,
    id: String,
    name: Option<String>,
    description: Option<String>,
    group_ids: Option<Vec<String>>,
    directory: Option<String>,
    body: Option<String>,
) -> RlResult<Skill> {
    mutate(&app, move |lib| {
        ops::skill_update_with_body(lib, &id, name, description, group_ids, directory, body)
    })
    .await
}

#[tauri::command]
pub async fn rl_skill_delete(app: AppHandle, id: String) -> RlResult<()> {
    mutate(&app, move |lib| ops::skill_delete(lib, &id)).await
}

// ── Skill 导入(文件夹 / zip 压缩包 / URL)────────────────────────────

#[tauri::command]
pub async fn rl_skill_import_folder(app: AppHandle, path: String) -> RlResult<SkillImportOutcome> {
    mutate(&app, move |lib| import::skill_import_folder(lib, &path)).await
}

#[tauri::command]
pub async fn rl_skill_import_archive(app: AppHandle, path: String) -> RlResult<SkillImportOutcome> {
    mutate(&app, move |lib| import::skill_import_archive(lib, &path)).await
}

#[tauri::command]
pub async fn rl_skill_import_url(app: AppHandle, url: String) -> RlResult<SkillImportOutcome> {
    mutate(&app, move |lib| import::skill_import_url(lib, &url)).await
}

#[tauri::command]
pub fn rl_skill_body_read(app: AppHandle, id: String) -> RlResult<SkillBody> {
    let lib = Library::app(&app)?;
    let _guard = lock_op();
    ops::body_read(&lib, &id)
}

#[tauri::command]
pub async fn rl_skill_body_write(app: AppHandle, id: String, content: String) -> RlResult<()> {
    mutate(&app, move |lib| ops::body_write(lib, &id, &content)).await
}

// ── Skill 预览:token 统计与安全扫描 ───────────────────────────────────

/// 技能 token 统计(与项目 AI 资产同口径的 o200k 估算):描述 + 全部文本文件
#[tauri::command]
pub async fn rl_skill_tokens(app: AppHandle, id: String) -> RlResult<SkillTokenReport> {
    let lib = Library::app(&app)?;
    blocking(move || {
        let _guard = lock_op();
        scan::token_report(&lib, &id)
    })
    .await
}

/// 技能目录绝对路径(图片预览走 asset 协议,前端据此拼文件 URL;`/` 分隔)
#[tauri::command]
pub async fn rl_skill_dir_path(app: AppHandle, id: String) -> RlResult<String> {
    let lib = Library::app(&app)?;
    blocking(move || {
        let _guard = lock_op();
        ops::skill_dir_path(&lib, &id).map(|p| crate::path_util::to_forward_slash(&p))
    })
    .await
}

/// 读取技能目录内单个文件(预览用;路径白名单校验,二进制/超限 content 为 None)
#[tauri::command]
pub async fn rl_skill_file_read(
    app: AppHandle,
    id: String,
    path: String,
) -> RlResult<SkillFileContent> {
    let lib = Library::app(&app)?;
    blocking(move || {
        let _guard = lock_op();
        scan::skill_file_read(&lib, &id, &path)
    })
    .await
}

/// 安全扫描的语义层模型:显式 provider/model 引用可解析时用所选模型,
/// 未选或引用失效(厂商/模型已删/密钥为空)回退 defaultModel —— 与 chat
/// 偏好的回退语义一致,选项展示由前端在配置加载后自行归位。两者都不可用
/// 返回 Err(调用方据此跳过语义层)。
fn resolve_scan_model(
    file: &crate::ai::catalog::AiConfigFile,
    provider_id: Option<&str>,
    model_id: Option<&str>,
) -> crate::error::AppResult<(crate::agent::llm::types::Model, String)> {
    if let (Some(provider_id), Some(model_id)) =
        (provider_id.map(str::trim), model_id.map(str::trim))
    {
        if !provider_id.is_empty() && !model_id.is_empty() {
            if let Ok(model) = crate::ai::catalog::resolve_model(file, provider_id, model_id) {
                if let Some(provider) = file.providers.get(provider_id) {
                    let api_key = provider.api_key.trim().to_string();
                    if !api_key.is_empty() {
                        return Ok((model, api_key));
                    }
                }
            }
        }
    }
    crate::ai::catalog::resolve_default_model(file)
}

/// 技能安全扫描(参考 SkillSpector 两层管线):静态规则层在进程锁内执行,
/// 语义层经内置 Agent(显式 provider_id/model_id 时用所选模型,缺省或引用
/// 失效回退设置页默认模型)在锁外调用;AI 未配置/失败/取消时静态结果照常
/// 返回,由 llm_status 标注。language 决定 AI 发现的输出语言。
#[tauri::command]
pub async fn rl_skill_scan(
    app: AppHandle,
    db: State<'_, Db>,
    id: String,
    language: Option<String>,
    run_id: Option<String>,
    provider_id: Option<String>,
    model_id: Option<String>,
) -> RlResult<SkillScanReport> {
    let lib = Library::app(&app)?;
    let run = run_id.map(RegisteredRun::new);
    let id_for_scan = id.clone();
    let (input, static_findings) = blocking(move || {
        let _guard = lock_op();
        let input = scan::load_scan_input(&lib, &id_for_scan)?;
        let findings = scan::static_findings(&input.files);
        Ok((input, findings))
    })
    .await?;
    execute_skill_scan(
        &app,
        &db,
        id,
        input,
        static_findings,
        language,
        run,
        provider_id,
        model_id,
    )
    .await
}

/// 扫描执行主体(库技能与本地目录共用):语义层模型解析、两层结果合并与评分
#[allow(clippy::too_many_arguments)]
async fn execute_skill_scan(
    app: &AppHandle,
    db: &Db,
    skill_id: String,
    input: scan::ScanInput,
    static_findings: Vec<SkillScanFinding>,
    language: Option<String>,
    run: Option<RegisteredRun>,
    provider_id: Option<String>,
    model_id: Option<String>,
) -> RlResult<SkillScanReport> {
    let has_scripts = scan::has_executable_script(&input.files);
    let language = language
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "zh-CN".to_string());
    let semantic = {
        let file = crate::ai::catalog::load_ai_config_file(app);
        resolve_scan_model(&file, provider_id.as_deref(), model_id.as_deref()).ok()
    };

    let mut findings = static_findings.clone();
    let mut llm_status = "skipped".to_string();
    let mut llm_error_code: Option<String> = None;
    let mut llm_error_message: Option<String> = None;
    let mut llm_summary: Option<String> = None;
    let mut suppressed = 0usize;

    if let Some((model, api_key)) = semantic {
        let system_prompt = fixed_system_prompt(DEFAULT_SKILL_SCAN_PROMPT, &language);
        let mut user_prompt =
            scan::build_llm_user_prompt(&input.name, &input.files, &static_findings, &language);
        let cancel_token = run.as_ref().map(|run| &run.token);
        let mut parsed: Option<scan::LlmReport> = None;
        // 解析或语言检查失败重试一次(provider 错误与取消不重试);
        // 每次尝试都是全新 harness 会话,usage 由 scan_agent 按 LLM 请求逐条落库
        for _attempt in 0..2 {
            let outcome = scan_agent::run_semantic_scan(
                db,
                model.clone(),
                &api_key,
                &input.dir,
                &system_prompt,
                &user_prompt,
                cancel_token,
            )
            .await;
            match outcome {
                Ok(text) => {
                    match scan::parse_llm_report(&text, static_findings.len()).and_then(|report| {
                        scan::validate_report_language(&report, &language)?;
                        Ok(report)
                    }) {
                        Ok(report) => {
                            parsed = Some(report);
                            break;
                        }
                        Err(error) => {
                            llm_status = "failed".to_string();
                            llm_error_code = None;
                            llm_error_message = Some(error);
                            if language == "zh-CN" {
                                user_prompt.push_str("\n上次输出未通过 JSON 或语言检查。请重新检查 JSON 结构，并确保摘要、每条风险标题和详情均使用简体中文，不得输出纯英文说明。");
                            }
                        }
                    }
                }
                Err(error) => {
                    if run.as_ref().is_some_and(|run| run.token.is_cancelled()) {
                        llm_status = "canceled".to_string();
                        llm_error_code = None;
                        llm_error_message = None;
                    } else {
                        llm_status = "failed".to_string();
                        llm_error_code = Some(error.code().to_string());
                        llm_error_message = Some(error.to_string());
                    }
                    break;
                }
            }
        }
        if let Some(llm) = parsed {
            llm_status = "ok".to_string();
            llm_error_code = None;
            llm_error_message = None;
            llm_summary = Some(llm.summary);
            let suppressed_set: HashSet<usize> = llm.false_positives.into_iter().collect();
            suppressed = suppressed_set.len();
            findings = static_findings
                .iter()
                .enumerate()
                .filter(|(index, _)| !suppressed_set.contains(index))
                .map(|(_, finding)| finding.clone())
                .collect();
            findings.extend(llm.findings);
        }
    }

    let (score, level) = scan::score_findings(&findings, has_scripts);
    Ok(SkillScanReport {
        skill_id,
        score,
        level,
        findings,
        static_count: static_findings.len(),
        suppressed_count: suppressed,
        files_scanned: input.files.len(),
        llm_status,
        llm_error_code,
        llm_error_message,
        llm_summary,
        scanned_at: crate::time_util::now_ts(),
    })
}

// ── 本地技能目录预览(项目内非托管技能;不读写资源库,无需进程锁)──────────

/// 本地技能目录报告:名称/描述(SKILL.md frontmatter)+ token 统计 + 内容指纹
#[tauri::command]
pub async fn skill_dir_overview(path: String) -> RlResult<SkillDirReport> {
    blocking(move || scan::dir_overview(Path::new(&path))).await
}

/// 读取本地技能目录内单个文件(预览用;与 rl_skill_file_read 同规则)
#[tauri::command]
pub async fn skill_dir_file_read(path: String, file: String) -> RlResult<SkillFileContent> {
    blocking(move || scan::dir_file_read(Path::new(&path), &file)).await
}

/// 本地技能目录安全扫描(与 rl_skill_scan 同一管线,共用扫描模型解析与取消机制)
#[tauri::command]
pub async fn skill_dir_scan(
    app: AppHandle,
    db: State<'_, Db>,
    path: String,
    language: Option<String>,
    run_id: Option<String>,
    provider_id: Option<String>,
    model_id: Option<String>,
) -> RlResult<SkillScanReport> {
    let run = run_id.map(RegisteredRun::new);
    let path_for_load = path.clone();
    let (input, static_findings) = blocking(move || {
        let input = scan::load_scan_input_at(Path::new(&path_for_load))?;
        let findings = scan::static_findings(&input.files);
        Ok((input, findings))
    })
    .await?;
    execute_skill_scan(
        &app,
        &db,
        path,
        input,
        static_findings,
        language,
        run,
        provider_id,
        model_id,
    )
    .await
}

// ── skills.sh 市场 ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn rl_marketplace_list(
    app: AppHandle,
    mode: String,
    query: Option<String>,
    source: Option<String>,
) -> RlResult<MarketplaceList> {
    let lib = Library::app(&app)?;
    blocking(move || {
        let _guard = lock_op();
        let mut result = if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
            marketplace::search(&query, source.as_deref())
        } else {
            marketplace::browse(&mode)
        }?;
        let local = ops::skill_list(&lib)?;
        for item in &mut result.skills {
            item.installed_skill_id = local.skills.iter().find_map(|skill| {
                skill
                    .marketplace
                    .as_ref()
                    .filter(|source| source.id == item.id)
                    .map(|_| skill.id.clone())
            });
        }
        Ok(result)
    })
    .await
}

#[tauri::command]
pub async fn rl_marketplace_install(app: AppHandle, id: String) -> RlResult<Skill> {
    mutate(&app, move |lib| {
        let download = marketplace::download(&id)?;
        let source = marketplace::source_for(&id, &download.repo_dir)?;
        // 安装基线:GitHub 查询失败不阻断安装,留 None 由检查更新时回填
        let installed_sha = marketplace::latest_commit_sha(&source.source, &source.repo_dir)
            .ok()
            .flatten();
        ops::skill_import_marketplace(lib, source, download, installed_sha)
    })
    .await
}

/// 逐技能检查市场更新;单技能失败只记录 error_code,不整体报错
#[tauri::command]
pub async fn rl_marketplace_check_updates(
    app: AppHandle,
) -> RlResult<Vec<MarketplaceUpdateStatus>> {
    let lib = Library::app(&app)?;
    blocking(move || {
        let _guard = lock_op();
        ops::skill_check_marketplace_updates(&lib)
    })
    .await
}

#[tauri::command]
pub async fn rl_marketplace_update_skill(app: AppHandle, id: String) -> RlResult<Skill> {
    mutate(&app, move |lib| {
        let data = ops::skill_list(lib)?;
        let marketplace = data
            .skills
            .iter()
            .find(|skill| skill.id == id)
            .and_then(|skill| skill.marketplace.clone())
            .ok_or_else(|| RlError::coded(codes::MARKETPLACE_SKILL_INVALID, id.clone()))?;
        let download = marketplace::download(&marketplace.id)?;
        let installed_sha = marketplace::latest_commit_sha(&marketplace.source, &download.repo_dir)
            .ok()
            .flatten();
        ops::skill_apply_marketplace_update(lib, &id, download, installed_sha)
    })
    .await
}

// ── 通用 MCP CRUD ──────────────────────────────────────────────────────

#[tauri::command]
pub fn rl_mcp_list(app: AppHandle) -> RlResult<Vec<McpServer>> {
    let lib = Library::app(&app)?;
    let _guard = lock_op();
    ops::mcp_list(&lib)
}

#[tauri::command]
pub async fn rl_mcp_create(app: AppHandle, def: McpServerInput) -> RlResult<McpServer> {
    mutate(&app, move |lib| ops::mcp_create(lib, &def)).await
}

#[tauri::command]
pub async fn rl_mcp_update(app: AppHandle, id: String, def: McpServerInput) -> RlResult<McpServer> {
    mutate(&app, move |lib| ops::mcp_update(lib, &id, &def)).await
}

#[tauri::command]
pub async fn rl_mcp_delete(app: AppHandle, id: String) -> RlResult<()> {
    mutate(&app, move |lib| ops::mcp_delete(lib, &id)).await
}

#[tauri::command]
pub async fn rl_mcp_import(
    app: AppHandle,
    defs: Vec<McpServerInput>,
) -> RlResult<McpImportOutcome> {
    mutate(&app, move |lib| ops::mcp_import(lib, &defs)).await
}

// ── 加密(可选,口令仅内存)──────────────────────────────────────────────

#[tauri::command]
pub async fn rl_encryption_enable(app: AppHandle, password: String) -> RlResult<SyncOutcome> {
    mutate_quiet(&app, move |lib| ops::encryption_enable(lib, &password)).await
}

#[tauri::command]
pub async fn rl_encryption_disable(app: AppHandle, password: String) -> RlResult<SyncOutcome> {
    mutate_quiet(&app, move |lib| ops::encryption_disable(lib, &password)).await
}

#[tauri::command]
pub async fn rl_encryption_unlock(app: AppHandle, password: String) -> RlResult<()> {
    mutate_quiet(&app, move |lib| ops::encryption_unlock(lib, &password)).await
}

#[tauri::command]
pub fn rl_encryption_lock(app: AppHandle) -> RlResult<()> {
    let lib = Library::app(&app)?;
    let _guard = lock_op();
    ops::encryption_lock(&lib);
    Ok(())
}

// ── Git / 同步 ─────────────────────────────────────────────────────────

/// 聚合 remote 配置:本地快照 → 设 URL → fetch →
/// 首次(本地为空)远端优先导入 / 纯快进 pull+push / 分叉透出待 resolve;
/// `branch` 指定远端分支名(远端为空时亦作为本地分支名)。网络失败不外抛。
#[tauri::command]
pub async fn rl_remote_configure(
    app: AppHandle,
    url: String,
    branch: Option<String>,
) -> RlResult<SyncOutcome> {
    let lib = Library::app(&app)?;
    let lib_work = lib.clone();
    let outcome = blocking(move || {
        let _guard = lock_op();
        git::remote_configure_impl(&lib_work, &url, branch)
    })
    .await?;
    record_and_emit(&app, &lib, &outcome).await;
    Ok(outcome)
}

#[tauri::command]
pub async fn rl_remote_remove(app: AppHandle) -> RlResult<()> {
    mutate_quiet(&app, git::remote_remove).await
}

#[tauri::command]
pub async fn rl_sync_status(app: AppHandle) -> RlResult<SyncStatus> {
    let lib = Library::app(&app)?;
    let _guard = SYNC_LOCK.lock().await;
    blocking(move || git::sync_status_impl(&lib)).await
}

#[tauri::command]
pub async fn rl_sync_once(app: AppHandle) -> RlResult<SyncOutcome> {
    let lib = Library::app(&app)?;
    let _guard = SYNC_LOCK.lock().await;
    let lib_run = lib.clone();
    let outcome = blocking(move || Ok(git::sync_once_impl(&lib_run))).await?;
    record_and_emit(&app, &lib, &outcome).await;
    Ok(outcome)
}

#[tauri::command]
pub async fn rl_resolve_fork(app: AppHandle, direction: String) -> RlResult<()> {
    let lib = Library::app(&app)?;
    let _guard = SYNC_LOCK.lock().await;
    blocking(move || git::resolve_fork(&lib, &direction)).await
}
