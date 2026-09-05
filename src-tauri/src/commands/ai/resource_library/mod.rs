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
mod marketplace;
mod models;
mod ops;
mod store;

#[cfg(test)]
mod tests;

pub use errors::{RlError, RlResult};
pub use models::*;

use std::sync::LazyLock;

use tauri::{AppHandle, Emitter};

use crate::error::{AppError, ErrorCode};

use models::{SyncOutcome, SyncRecord};
use store::{lock_op, Library};

/// 后台自动同步完成事件(负载为 SyncOutcome)
pub const SYNC_EVENT: &str = "resource-library://sync-completed";

/// 同步互斥:后台自动同步与显式同步命令全局串行
static SYNC_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

fn spawn_err(e: tokio::task::JoinError) -> RlError {
    RlError::App(AppError::coded(ErrorCode::GitTaskFailed, e.to_string()))
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
    color: Option<String>,
) -> RlResult<SkillGroup> {
    mutate(&app, move |lib| ops::group_create(lib, &name, color)).await
}

#[tauri::command]
pub async fn rl_skill_group_rename(
    app: AppHandle,
    id: String,
    name: String,
    color: Option<String>,
) -> RlResult<SkillGroup> {
    mutate(&app, move |lib| ops::group_rename(lib, &id, &name, color)).await
}

#[tauri::command]
pub async fn rl_skill_group_delete(app: AppHandle, id: String) -> RlResult<()> {
    mutate(&app, move |lib| ops::group_delete(lib, &id)).await
}

#[tauri::command]
pub async fn rl_skill_group_reorder(app: AppHandle, ids: Vec<String>) -> RlResult<()> {
    mutate(&app, move |lib| ops::group_reorder(lib, &ids)).await
}

#[tauri::command]
pub async fn rl_skill_reorder(app: AppHandle, ids: Vec<String>) -> RlResult<()> {
    mutate(&app, move |lib| ops::skill_reorder(lib, &ids)).await
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
        let (source, body) = marketplace::download(&id)?;
        ops::skill_import_marketplace(lib, source, body)
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
