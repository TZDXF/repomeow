mod models;
mod process;

use std::path::PathBuf;
use std::sync::OnceLock;

use tauri::AppHandle;

use crate::commands::git::open_repo;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;

use models::SemCliEnvelope;
pub use models::{SemanticDiffResult, SemanticStatus};
use process::run_sem;

static SEM_VERSION: OnceLock<String> = OnceLock::new();

fn output_error(code: Option<i32>, stderr: &[u8]) -> AppError {
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

async fn detect_version(app: &AppHandle) -> AppResult<String> {
    if let Some(version) = SEM_VERSION.get() {
        return Ok(version.clone());
    }
    let output = run_sem(app, None, &["--version".to_string()]).await?;
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

fn resolve_commit_root(path: &str, hash: &str) -> AppResult<(PathBuf, String)> {
    let normalized = clean_str(path);
    let Some(repo) = open_repo(&normalized)? else {
        return Err(AppError::coded(ErrorCode::NotGitRepository, normalized));
    };
    let commit = repo
        .revparse_single(hash.trim())
        .and_then(|object| object.peel_to_commit())
        .map_err(|error| {
            AppError::coded(
                ErrorCode::SemanticToolFailed,
                format!("commit={} detail={error}", hash.trim()),
            )
        })?;
    let root = repo
        .workdir()
        .ok_or_else(|| AppError::coded(ErrorCode::SemanticToolFailed, "bare repository"))?;
    Ok((root.to_path_buf(), commit.id().to_string()))
}

#[tauri::command]
pub async fn semantic_status(app: AppHandle) -> AppResult<SemanticStatus> {
    Ok(SemanticStatus {
        version: detect_version(&app).await?,
    })
}

#[tauri::command]
pub async fn semantic_commit_diff(
    app: AppHandle,
    path: String,
    hash: String,
) -> AppResult<SemanticDiffResult> {
    let (root, oid) = resolve_commit_root(&path, &hash)?;
    let version = detect_version(&app).await?;
    let args = vec![
        "diff".to_string(),
        "--commit".to_string(),
        oid,
        "--format".to_string(),
        "json".to_string(),
    ];
    let output = run_sem(&app, Some(&root), &args).await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let envelope: SemCliEnvelope = serde_json::from_slice(&output.stdout)
        .map_err(|error| AppError::coded(ErrorCode::SemanticOutputInvalid, error.to_string()))?;
    Ok(SemanticDiffResult {
        engine_version: version,
        summary: envelope.summary,
        changes: envelope.changes.into_iter().map(Into::into).collect(),
        binary_changes: envelope.binary_changes,
    })
}

pub use process::cleanup_on_exit;

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
}
