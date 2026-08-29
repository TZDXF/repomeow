mod context;
mod history;
mod impact;
mod models;
mod navigation;
mod parse;
mod process;

use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use tauri::AppHandle;

use crate::commands::git::open_repo;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::{clean_str, to_forward_slash_str};

#[cfg(test)]
use models::SemCliEnvelope;
pub use models::{
    SemanticContextResult, SemanticDiffResult, SemanticEntityLogResult, SemanticFileBlameResult,
    SemanticFileEntitiesResult, SemanticFindResult, SemanticImpactResult, SemanticRelationResult,
    SemanticStatus,
};
use process::{run_sem, SemRunPolicy};

static SEM_VERSION: OnceLock<String> = OnceLock::new();

pub(super) fn output_error(code: Option<i32>, stderr: &[u8]) -> AppError {
    let detail = String::from_utf8_lossy(stderr).trim().to_string();
    AppError::coded(
        ErrorCode::SemanticToolFailed,
        if detail.is_empty() {
            format!(
                "exit={}",
                code.map_or_else(|| "unknown".to_string(), |v| v.to_string())
            )
        } else {
            format!(
                "exit={} stderr={detail}",
                code.map_or_else(|| "unknown".to_string(), |v| v.to_string())
            )
        },
    )
}

pub(super) async fn detect_version(app: &AppHandle) -> AppResult<String> {
    if let Some(version) = SEM_VERSION.get() {
        return Ok(version.clone());
    }
    let output = run_sem(
        app,
        None,
        &["--version".to_string()],
        SemRunPolicy::DEFAULT,
        None,
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let raw = String::from_utf8(output.stdout)
        .map_err(|error| AppError::coded(ErrorCode::SemanticOutputInvalid, error.to_string()))?;
    let version = raw
        .trim()
        .strip_prefix("sem ")
        .unwrap_or(raw.trim())
        .trim()
        .to_string();
    if version.is_empty() {
        return Err(AppError::coded(
            ErrorCode::SemanticOutputInvalid,
            "empty version output",
        ));
    }
    let _ = SEM_VERSION.set(version.clone());
    Ok(version)
}

/// 解析项目路径为 sem 的工作目录(实际仓库 workdir)。
pub(super) fn resolve_workdir(path: &str) -> AppResult<PathBuf> {
    let normalized = clean_str(path);
    let Some(repo) = open_repo(&normalized)? else {
        return Err(AppError::coded(ErrorCode::NotGitRepository, normalized));
    };
    let root = repo
        .workdir()
        .ok_or_else(|| AppError::coded(ErrorCode::SemanticToolFailed, "bare repository"))?;
    Ok(root.to_path_buf())
}

/// 校验并归一化仓库内相对路径:仅接受 `/` 分隔(反斜杠宽容归一),拒绝绝对
/// 路径、`..`、空路径与 NUL;存在时 canonicalize 后必须仍位于 workdir 内。
/// 返回值恒为 `/` 分隔。
pub(super) fn validate_rel_file_path(root: &Path, file_path: &str) -> AppResult<String> {
    let invalid = || AppError::coded(ErrorCode::InvalidPath, file_path.to_string());
    let trimmed = file_path.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(invalid());
    }
    let normalized = to_forward_slash_str(trimmed);
    let candidate = Path::new(&normalized);
    // 显式拒绝 Windows 盘符(在非 Windows 上 components 不会拆出 Prefix)。
    let bytes = normalized.as_bytes();
    let has_drive_letter = bytes.len() >= 2
        && bytes[1] == b':'
        && bytes[0].is_ascii_alphabetic();
    if candidate.is_absolute() || normalized.starts_with('/') || has_drive_letter {
        return Err(invalid());
    }
    for component in candidate.components() {
        if !matches!(component, Component::Normal(_) | Component::CurDir) {
            return Err(invalid());
        }
    }
    let joined = root.join(&normalized);
    if joined.exists() {
        let canonical_root = root
            .canonicalize()
            .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
        let canonical = joined
            .canonicalize()
            .map_err(|error| AppError::coded(ErrorCode::InvalidPath, error.to_string()))?;
        if !canonical.starts_with(&canonical_root) {
            return Err(invalid());
        }
    }
    Ok(normalized)
}

/// 校验搜索 query:非空、无 NUL、限长(字符数)。
pub(super) fn validate_query(query: &str) -> AppResult<String> {
    let invalid = || {
        AppError::coded(
            ErrorCode::SemanticToolFailed,
            format!("invalid query (empty/NUL/>200 chars)"),
        )
    };
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.chars().count() > 200 {
        return Err(invalid());
    }
    Ok(trimmed.to_string())
}

/// 校验实体名 / entityId:非空、无 NUL、限长。
pub(super) fn validate_entity_token(value: &str) -> AppResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('\0') || trimmed.chars().count() > 500 {
        return Err(AppError::coded(
            ErrorCode::SemanticToolFailed,
            "invalid entity id/name".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

#[tauri::command]
pub async fn semantic_status(app: AppHandle) -> AppResult<SemanticStatus> {
    Ok(SemanticStatus {
        version: detect_version(&app).await?,
    })
}

/// 取消进行中的语义分析请求(前端每次新请求生成 requestId)。
/// 返回是否存在该请求;不存在视为已结束,不算错误。
#[tauri::command]
pub async fn semantic_cancel(request_id: String) -> AppResult<bool> {
    Ok(process::cancel_request(&request_id))
}

// ── 第 2 期:语义导航 ────────────────────────────────────────────────

#[tauri::command]
pub async fn semantic_file_entities(
    app: AppHandle,
    path: String,
    file_path: String,
    request_id: Option<String>,
) -> AppResult<SemanticFileEntitiesResult> {
    navigation::file_entities_impl(app, path, file_path, request_id).await
}

#[tauri::command]
pub async fn semantic_find_entities(
    app: AppHandle,
    path: String,
    query: String,
    request_id: Option<String>,
) -> AppResult<SemanticFindResult> {
    navigation::find_entities_impl(app, path, query, request_id).await
}

#[tauri::command]
pub async fn semantic_entity_callers(
    app: AppHandle,
    path: String,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    request_id: Option<String>,
) -> AppResult<SemanticRelationResult> {
    navigation::entity_relation_impl(
        app,
        path,
        navigation::RelationKind::Callers,
        entity_id,
        entity_name,
        file_path,
        request_id,
    )
    .await
}

#[tauri::command]
pub async fn semantic_entity_refs(
    app: AppHandle,
    path: String,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    request_id: Option<String>,
) -> AppResult<SemanticRelationResult> {
    navigation::entity_relation_impl(
        app,
        path,
        navigation::RelationKind::Refs,
        entity_id,
        entity_name,
        file_path,
        request_id,
    )
    .await
}

// ── 第 3 期:影响分析 ────────────────────────────────────────────────

#[tauri::command]
pub async fn semantic_entity_impact(
    app: AppHandle,
    path: String,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    depth: Option<usize>,
    request_id: Option<String>,
) -> AppResult<SemanticImpactResult> {
    impact::entity_impact_impl(app, path, entity_id, entity_name, file_path, depth, request_id)
        .await
}

// ── 第 4A 期:实体历史 ───────────────────────────────────────────────

#[tauri::command]
pub async fn semantic_file_blame(
    app: AppHandle,
    path: String,
    file_path: String,
    request_id: Option<String>,
) -> AppResult<SemanticFileBlameResult> {
    history::file_blame_impl(app, path, file_path, request_id).await
}

#[tauri::command]
pub async fn semantic_entity_log(
    app: AppHandle,
    path: String,
    entity_name: String,
    file_path: Option<String>,
    limit: Option<usize>,
    request_id: Option<String>,
) -> AppResult<SemanticEntityLogResult> {
    history::entity_log_impl(app, path, entity_name, file_path, limit, request_id).await
}

// ── 第 4B 期:AI 语义上下文 ──────────────────────────────────────────

#[tauri::command]
pub async fn semantic_worktree_diff(
    app: AppHandle,
    path: String,
    request_id: Option<String>,
) -> AppResult<SemanticDiffResult> {
    context::worktree_diff_impl(app, path, request_id).await
}

#[tauri::command]
pub async fn semantic_entity_context(
    app: AppHandle,
    path: String,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    budget: Option<usize>,
    hops: Option<usize>,
    request_id: Option<String>,
) -> AppResult<SemanticContextResult> {
    context::entity_context_impl(
        app, path, entity_id, entity_name, file_path, budget, hops, request_id,
    )
    .await
}

pub use process::cleanup_on_exit;
pub(crate) use context::commit_input_analysis;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sem_json_and_drops_entity_contents() {
        let raw = br#"{
          "summary":{"fileCount":1,"modified":1,"total":1},
          "changes":[{
            "entityId":"src/a.ts::function::run",
            "changeType":"modified",
            "entityType":"function",
            "entityName":"run",
            "startLine":3,
            "endLine":5,
            "oldStartLine":2,
            "oldEndLine":4,
            "filePath":"src/a.ts",
            "beforeContent":"secret before",
            "afterContent":"secret after",
            "structuralChange":true,
            "futureField":"ignored"
          }],
          "binaryChanges":[]
        }"#;
        let parsed: SemCliEnvelope = serde_json::from_slice(raw).unwrap();
        assert_eq!(parsed.summary.modified, 1);
        let change: models::SemanticChange = parsed.changes.into_iter().next().unwrap().into();
        assert_eq!(change.entity_name, "run");
        assert_eq!(change.start_line, 3);
        assert_eq!(change.structural_change, Some(true));
        let value = serde_json::to_value(change).unwrap();
        assert!(value.get("beforeContent").is_none());
        assert!(value.get("afterContent").is_none());
    }

    #[test]
    fn accepts_missing_optional_summary_and_change_fields() {
        let raw = br#"{
          "summary":{"fileCount":0,"total":0},
          "changes":[],
          "binaryChanges":[{
            "changeType":"binary",
            "filePath":"logo.png",
            "oldFilePath":null,
            "fileStatus":"modified"
          }]
        }"#;
        let parsed: SemCliEnvelope = serde_json::from_slice(raw).unwrap();
        assert_eq!(parsed.summary.added, 0);
        assert_eq!(parsed.binary_changes.len(), 1);
    }

    #[test]
    fn rejects_out_of_repo_file_paths() {
        let root = Path::new(".");
        for bad in [
            "",
            "  ",
            "../outside.ts",
            "src/../../outside.ts",
            "/etc/passwd",
            "C:/Windows/system.ini",
            "src/lib\0.ts",
        ] {
            assert!(
                validate_rel_file_path(root, bad).is_err(),
                "should reject: {bad:?}"
            );
        }
        let ok = validate_rel_file_path(root, "src/lib/utils.ts").unwrap();
        assert_eq!(ok, "src/lib/utils.ts");
        let backslash = validate_rel_file_path(root, "src\\lib\\utils.ts").unwrap();
        assert_eq!(backslash, "src/lib/utils.ts");
    }

    #[test]
    fn rejects_invalid_query_and_entity_token() {
        assert!(validate_query("").is_err());
        assert!(validate_query("   ").is_err());
        assert!(validate_query("a\0b").is_err());
        assert!(validate_query(&"x".repeat(201)).is_err());
        assert_eq!(validate_query("  debounce ").unwrap(), "debounce");
        assert!(validate_entity_token("").is_err());
        assert!(validate_entity_token(&"x".repeat(501)).is_err());
        assert_eq!(
            validate_entity_token("src/a.ts::function::run").unwrap(),
            "src/a.ts::function::run"
        );
    }
}
