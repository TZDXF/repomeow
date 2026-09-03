//! 实体历史:blame / log(第 4A 期)。

use serde::Deserialize;

use crate::error::{AppError, AppResult, ErrorCode};

use super::models::{
    SemanticBlameEntry, SemanticEntityLogChange, SemanticEntityLogResult, SemanticFileBlameResult,
};
use super::parse::{parse_stdout, truncate_to};
use super::process::{run_sem, SemLauncher, SemRunPolicy};
use super::{
    detect_version, output_error, resolve_workdir, validate_entity_token, validate_rel_file_path,
};

/// 单文件 blame 条目硬上限
const BLAME_LIMIT: usize = 2_000;
/// log 条数:缺省 50,clamp 1..=100
const LOG_DEFAULT_LIMIT: usize = 50;
const LOG_MAX_LIMIT: usize = 100;

// ── sem 0.23.1 原始 JSON(未知字段默认忽略)────────────────────────────

#[derive(Debug, Deserialize)]
struct RawBlameEntry {
    author: String,
    commit: String,
    date: String,
    #[serde(default)]
    lines: [usize; 2],
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize)]
struct RawLogCommit {
    #[serde(default)]
    author: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    message: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct RawLogChange {
    change_type: String,
    #[serde(default)]
    structural_change: Option<bool>,
    #[serde(default)]
    file_path: String,
    commit: RawLogCommit,
}

#[derive(Debug, Deserialize)]
struct RawLogOutput {
    entity: String,
    #[serde(rename = "type")]
    entity_type: String,
    file: String,
    #[serde(default)]
    changes: Vec<RawLogChange>,
}

/// stderr 中的「实体不存在」判定(sem 文案:`error: Entity 'x' not found ...`)。
fn is_entity_not_found(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("not found")
}

/// log 条数:缺省 50,越界 clamp 到 1..=100。
fn clamp_log_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(LOG_DEFAULT_LIMIT).clamp(1, LOG_MAX_LIMIT)
}

pub(super) async fn file_blame_impl(
    launcher: SemLauncher,
    path: String,
    file_path: String,
    request_id: Option<String>,
) -> AppResult<SemanticFileBlameResult> {
    let root = resolve_workdir(&path)?;
    let file = validate_rel_file_path(&root, &file_path)?;
    let version = detect_version(&launcher).await?;
    let args = vec!["blame".to_string(), file.clone(), "--json".to_string()];
    let output = run_sem(
        &launcher,
        Some(&root),
        &args,
        SemRunPolicy::HEAVY,
        request_id.as_deref(),
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let raw: Vec<RawBlameEntry> = parse_stdout(&output.stdout)?;
    let mut entries: Vec<SemanticBlameEntry> = raw
        .into_iter()
        .map(|item| SemanticBlameEntry {
            name: item.name,
            entity_type: item.entity_type,
            start_line: item.lines[0],
            end_line: item.lines[1],
            author: item.author,
            commit: item.commit,
            date: item.date,
            summary: item.summary,
        })
        .collect();
    let truncated = truncate_to(&mut entries, BLAME_LIMIT);
    Ok(SemanticFileBlameResult {
        engine_version: version,
        file_path: file,
        entries,
        truncated,
    })
}

pub(super) async fn entity_log_impl(
    launcher: SemLauncher,
    path: String,
    entity_name: String,
    file_path: Option<String>,
    limit: Option<usize>,
    request_id: Option<String>,
) -> AppResult<SemanticEntityLogResult> {
    let root = resolve_workdir(&path)?;
    let name = validate_entity_token(&entity_name)?;
    let limit = clamp_log_limit(limit);
    let mut args = vec![
        "log".to_string(),
        name,
        "--limit".to_string(),
        limit.to_string(),
        "--json".to_string(),
    ];
    if let Some(file) = file_path {
        args.push("--file".to_string());
        args.push(validate_rel_file_path(&root, &file)?);
    }
    let version = detect_version(&launcher).await?;
    let output = run_sem(
        &launcher,
        Some(&root),
        &args,
        SemRunPolicy::HEAVY,
        request_id.as_deref(),
    )
    .await?;
    if output.code != Some(0) {
        if is_entity_not_found(&output.stderr) {
            return Err(AppError::coded(
                ErrorCode::SemanticEntityNotFound,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        return Err(output_error(output.code, &output.stderr));
    }
    let raw: RawLogOutput = parse_stdout(&output.stdout)?;
    let changes: Vec<SemanticEntityLogChange> = raw
        .changes
        .into_iter()
        .map(|change| SemanticEntityLogChange {
            change_type: change.change_type,
            structural_change: change.structural_change,
            file_path: change.file_path,
            commit_sha: change.commit.sha,
            author: change.commit.author,
            date: change.commit.date,
            message: change.commit.message,
        })
        .collect();
    Ok(SemanticEntityLogResult {
        engine_version: version,
        entity: raw.entity,
        entity_type: raw.entity_type,
        file_path: raw.file,
        // limit 语义由 sem 自己执行,返回条数即全部,无额外截断
        truncated: false,
        changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blame_entries() {
        // 契约来自 sem 0.23.1 `blame src/lib/utils.ts --json` 真实输出(缩写)
        let raw = br#"[
          {"author":"TZDXF","commit":"51ffa253307918c73b3dd300549f9261cd0f0a2f","date":"2026-07-16","lines":[6,8],"name":"cn","summary":"scaffold","type":"function"},
          {"author":"TZDXF","commit":"b656b00fd37e60a50cd00e8e83004323e71724cd","date":"2026-08-22","lines":[11,18],"name":"copyToClipboard","summary":"refactor","type":"function"}
        ]"#;
        let parsed: Vec<RawBlameEntry> = parse_stdout(raw).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].commit.len(), 40);
        assert!(parsed[0].commit.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(parsed[1].lines, [11, 18]);
        // 缺 summary 宽容
        let sparse: Vec<RawBlameEntry> = parse_stdout(
            br#"[{"author":"a","commit":"b656b00fd37e60a50cd00e8e83004323e71724cd","date":"2026-01-01","name":"x","type":"function"}]"#,
        )
        .unwrap();
        assert_eq!(sparse[0].summary, "");
        assert_eq!(sparse[0].lines, [0, 0]);
    }

    #[test]
    fn parses_entity_log_changes() {
        // 契约来自 sem 0.23.1 `log debounce --file ... --json` 真实输出(缩写)
        let raw = br#"{
          "entity":"debounce","file":"src/lib/utils.ts","type":"function",
          "changes":[
            {"change_type":"added","commit":{"author":"TZDXF","date":"2026-08-22","message":"refactor","sha":"b656b00fd37e60a50cd00e8e83004323e71724cd"},"file_path":"src/lib/utils.ts","structural_change":true},
            {"change_type":"modified","commit":{"author":"TZDXF","date":"2026-08-23","message":"fix","sha":"51ffa253307918c73b3dd300549f9261cd0f0a2f"},"file_path":"src/lib/utils.ts"}
          ]
        }"#;
        let parsed: RawLogOutput = parse_stdout(raw).unwrap();
        assert_eq!(parsed.entity, "debounce");
        assert_eq!(parsed.changes.len(), 2);
        assert_eq!(parsed.changes[0].structural_change, Some(true));
        // structural_change 缺省为 None
        assert_eq!(parsed.changes[1].structural_change, None);
        assert_eq!(parsed.changes[0].commit.sha.len(), 40);
    }

    #[test]
    fn log_limit_clamps_to_bounds() {
        assert_eq!(clamp_log_limit(None), 50);
        assert_eq!(clamp_log_limit(Some(0)), 1);
        assert_eq!(clamp_log_limit(Some(500)), 100);
        assert_eq!(clamp_log_limit(Some(30)), 30);
    }

    #[test]
    fn entity_not_found_detected_from_stderr() {
        assert!(is_entity_not_found(
            b"error: Entity 'zzz' not found in scanned history of src/lib/utils.ts"
        ));
        assert!(!is_entity_not_found(b"error: other"));
    }
}
