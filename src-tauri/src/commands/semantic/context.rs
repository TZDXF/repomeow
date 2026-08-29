//! AI 语义上下文:工作区 diff 摘要 / 单实体 context(第 4B 期)。

use serde::Deserialize;
use tauri::AppHandle;

use crate::commands::git::open_repo;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;

use super::models::{
    SemanticContextEntry, SemanticContextOmitted, SemanticContextResult, SemanticDiffResult,
};
use super::parse::parse_stdout;
use super::process::{run_sem, SemRunPolicy};
use super::{detect_version, output_error, resolve_workdir, validate_entity_token, validate_rel_file_path};

/// context token 预算:缺省 2000,clamp 500..=4000
const CONTEXT_DEFAULT_BUDGET: usize = 2_000;
const CONTEXT_MIN_BUDGET: usize = 500;
const CONTEXT_MAX_BUDGET: usize = 4_000;
/// context 关系跳数:缺省 1,clamp 0..=3
const CONTEXT_DEFAULT_HOPS: usize = 1;
const CONTEXT_MAX_HOPS: usize = 3;

// ── sem 0.23.1 原始 JSON(未知字段默认忽略)────────────────────────────

use super::models::SemCliEnvelope;

#[derive(Debug, Deserialize)]
struct RawContextEntry {
    #[serde(rename = "entityId")]
    entity_id: String,
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    file: String,
    role: String,
    #[serde(default)]
    tokens: usize,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RawContextOmitted {
    #[serde(default)]
    role: String,
    #[serde(default)]
    entities: usize,
    #[serde(default)]
    tests: usize,
}

#[derive(Debug, Deserialize)]
struct RawContextOutput {
    entity: String,
    #[serde(rename = "entityId", default)]
    entity_id: String,
    #[serde(default)]
    entries: Vec<RawContextEntry>,
    #[serde(default)]
    omitted: Vec<RawContextOmitted>,
    #[serde(default)]
    target_omitted: bool,
    #[serde(default)]
    total_tokens: usize,
    #[serde(default)]
    truncated: bool,
}

/// stderr 中的「实体不存在」判定。
fn is_entity_not_found(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("not found")
}

fn clamp_budget(budget: Option<usize>) -> usize {
    budget
        .unwrap_or(CONTEXT_DEFAULT_BUDGET)
        .clamp(CONTEXT_MIN_BUDGET, CONTEXT_MAX_BUDGET)
}

fn clamp_hops(hops: Option<usize>) -> usize {
    hops.unwrap_or(CONTEXT_DEFAULT_HOPS).min(CONTEXT_MAX_HOPS)
}

/// 工作区语义 diff:HEAD → index+worktree(仅已跟踪文件,口径同 git_commit_context);
/// 仓库尚无 HEAD(unborn)时回退到暂存区 diff(相对空树)。
pub(super) async fn worktree_diff_impl(
    app: AppHandle,
    path: String,
    request_id: Option<String>,
) -> AppResult<SemanticDiffResult> {
    worktree_diff(&app, &path, request_id.as_deref()).await
}

pub(super) async fn worktree_diff(
    app: &AppHandle,
    path: &str,
    request_id: Option<&str>,
) -> AppResult<SemanticDiffResult> {
    let normalized = clean_str(path);
    let Some(repo) = open_repo(&normalized)? else {
        return Err(AppError::coded(ErrorCode::NotGitRepository, normalized));
    };
    let has_head = !matches!(repo.head(), Err(e) if e.code() == git2::ErrorCode::UnbornBranch);
    let root = repo
        .workdir()
        .ok_or_else(|| AppError::coded(ErrorCode::SemanticToolFailed, "bare repository"))?
        .to_path_buf();

    let mut args = vec!["diff".to_string()];
    if has_head {
        args.push("HEAD".to_string());
    } else {
        args.push("--staged".to_string());
    }
    args.push("--format".to_string());
    args.push("json".to_string());

    let version = detect_version(&app).await?;
    let output = run_sem(
        &app,
        Some(&root),
        &args,
        SemRunPolicy::CONTEXT,
        request_id.as_deref(),
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    // SemCliEnvelope 解析即丢弃 beforeContent/afterContent,实体全文不进 IPC
    let envelope: SemCliEnvelope = parse_stdout(&output.stdout)?;
    Ok(SemanticDiffResult {
        engine_version: version,
        summary: envelope.summary,
        changes: envelope.changes.into_iter().map(Into::into).collect(),
        binary_changes: envelope.binary_changes,
    })
}

/// 单实体的 token 预算上下文(sem context)。entries[].content 是源码片段,
/// 仅在用户显式触发后经 IPC 返回,不落库、不写日志。
pub(super) async fn entity_context_impl(
    app: AppHandle,
    path: String,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    budget: Option<usize>,
    hops: Option<usize>,
    request_id: Option<String>,
) -> AppResult<SemanticContextResult> {
    let root = resolve_workdir(&path)?;
    let budget = clamp_budget(budget);
    let hops = clamp_hops(hops);

    let mut args = vec!["context".to_string()];
    match entity_id {
        Some(id) => {
            args.push("--entity-id".to_string());
            args.push(validate_entity_token(&id)?);
        }
        None => {
            let name = entity_name.ok_or_else(|| {
                AppError::coded(
                    ErrorCode::SemanticToolFailed,
                    "entity_id/entity_name both empty".to_string(),
                )
            })?;
            args.push(validate_entity_token(&name)?);
        }
    }
    if let Some(file) = file_path {
        args.push("--file".to_string());
        args.push(validate_rel_file_path(&root, &file)?);
    }
    args.push("--budget".to_string());
    args.push(budget.to_string());
    args.push("--hops".to_string());
    args.push(hops.to_string());
    args.push("--json".to_string());

    let version = detect_version(&app).await?;
    let output = run_sem(
        &app,
        Some(&root),
        &args,
        SemRunPolicy::CONTEXT,
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

    let raw: RawContextOutput = parse_stdout(&output.stdout)?;
    Ok(SemanticContextResult {
        engine_version: version,
        entity: raw.entity,
        entity_id: raw.entity_id,
        budget,
        total_tokens: raw.total_tokens,
        truncated: raw.truncated,
        target_omitted: raw.target_omitted,
        entries: raw
            .entries
            .into_iter()
            .map(|entry| SemanticContextEntry {
                entity_id: entry.entity_id,
                name: entry.name,
                entity_type: entry.entity_type,
                file_path: entry.file,
                role: entry.role,
                tokens: entry.tokens,
                content: entry.content,
            })
            .collect(),
        omitted: raw
            .omitted
            .into_iter()
            .map(|group| SemanticContextOmitted {
                role: group.role,
                entities: group.entities,
                tests: group.tests,
            })
            .collect(),
    })
}

/// 提交信息的紧凑语义摘要预算(字符)
const DIFF_SUMMARY_BUDGET: usize = 6_000;
/// 摘要中的实体条数上限(防御;预算通常先触顶)
const DIFF_SUMMARY_MAX_ITEMS: usize = 200;

/// 把工作区语义 diff 压成紧凑摘要(每行:路径 + changeType + 实体类型/名称 +
/// structural/cosmetic 标记),结构变化优先、cosmetic 后置,按字符预算截断。
/// 摘要是 AI 提示词的结构线索,raw diff 仍是最终事实证据。
pub(super) fn build_diff_summary(result: &SemanticDiffResult, budget: usize) -> String {
    let mut structural: Vec<String> = Vec::new();
    let mut cosmetic: Vec<String> = Vec::new();
    for change in &result.changes {
        let is_cosmetic = change.structural_change == Some(false);
        let mark = if is_cosmetic { "cosmetic" } else { "structural" };
        let line = format!(
            "{}: {} {} {} ({mark})",
            change.file_path, change.change_type, change.entity_type, change.entity_name
        );
        if is_cosmetic {
            cosmetic.push(line);
        } else {
            structural.push(line);
        }
    }
    for binary in &result.binary_changes {
        structural.push(format!(
            "{}: binary {} ({})",
            binary.file_path, binary.file_status, binary.change_type
        ));
    }
    let mut out = String::new();
    let mut count = 0usize;
    let mut truncated = false;
    for line in structural.into_iter().chain(cosmetic) {
        if count >= DIFF_SUMMARY_MAX_ITEMS || out.len() + line.len() + 1 > budget {
            truncated = true;
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
        count += 1;
    }
    if truncated {
        out.push_str("\n... (semantic summary truncated)");
    }
    out
}

/// AI 提交信息的语义摘要入口:sem 失败/超时/无实体时静默回退(None),
/// 调用方保持原有 prompt,不让「生成提交信息」失败。
pub(crate) async fn worktree_diff_summary(app: &AppHandle, path: &str) -> Option<String> {
    let result = worktree_diff(app, path, None).await.ok()?;
    if result.changes.is_empty() && result.binary_changes.is_empty() {
        return None;
    }
    let summary = build_diff_summary(&result, DIFF_SUMMARY_BUDGET);
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::semantic::models::SemanticChange;

    #[test]
    fn worktree_diff_envelope_drops_entity_contents() {
        // 契约来自 sem 0.23.1 `diff --json` 真实输出(缩写)
        let raw = br#"{
          "summary":{"fileCount":1,"added":1,"total":1},
          "changes":[{"entityId":"src/lib/utils.ts::orphan::added@L30-31","changeType":"added","entityType":"orphan","entityName":"module-level","startLine":30,"endLine":31,"oldStartLine":null,"oldEndLine":null,"oldEntityName":null,"filePath":"src/lib/utils.ts","oldFilePath":null,"oldParentId":null,"beforeContent":null,"afterContent":"\n// probe","commitSha":null,"author":null,"structuralChange":false}],
          "binaryChanges":[]
        }"#;
        let envelope: SemCliEnvelope = parse_stdout(raw).unwrap();
        let change: SemanticChange = envelope.changes.into_iter().next().unwrap().into();
        let value = serde_json::to_value(&change).unwrap();
        assert!(value.get("beforeContent").is_none());
        assert!(value.get("afterContent").is_none());
        assert_eq!(change.entity_type, "orphan");
        assert_eq!(change.structural_change, Some(false));
    }

    #[test]
    fn parses_context_output_with_omitted_groups() {
        // 契约来自 sem 0.23.1 `context --entity-id ... --json` 真实输出(缩写)
        let raw = br#"{
          "budget":200,"entity":"now_ts","entityId":"src-tauri/src/time_util.rs::function::now_ts",
          "entries":[{"content":"pub fn now_ts()","entityId":"src-tauri/src/time_util.rs::function::now_ts","file":"src-tauri/src/time_util.rs","name":"now_ts","role":"target","tokens":10,"type":"function"}],
          "omitted":[{"entities":8,"role":"direct_dependent","tests":8}],
          "target_omitted":false,"total_tokens":10,"truncated":true
        }"#;
        let parsed: RawContextOutput = parse_stdout(raw).unwrap();
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].role, "target");
        assert_eq!(parsed.omitted[0].entities, 8);
        assert_eq!(parsed.omitted[0].tests, 8);
        assert!(parsed.truncated);
        assert!(!parsed.target_omitted);
    }

    #[test]
    fn context_tolerates_missing_optional_fields() {
        let raw = br#"{"entity":"x","entries":[],"omitted":[]}"#;
        let parsed: RawContextOutput = parse_stdout(raw).unwrap();
        assert_eq!(parsed.entity_id, "");
        assert!(!parsed.truncated);
        assert_eq!(parsed.total_tokens, 0);
    }

    #[test]
    fn budget_and_hops_clamp_to_bounds() {
        assert_eq!(clamp_budget(None), 2000);
        assert_eq!(clamp_budget(Some(1)), 500);
        assert_eq!(clamp_budget(Some(99999)), 4000);
        assert_eq!(clamp_hops(None), 1);
        assert_eq!(clamp_hops(Some(0)), 0);
        assert_eq!(clamp_hops(Some(9)), 3);
    }

    #[test]
    fn entity_not_found_detected_from_stderr() {
        assert!(is_entity_not_found(b"error: Entity 'zzz' not found"));
        assert!(!is_entity_not_found(b"error: other"));
    }

    fn change(structural: Option<bool>, name: &str) -> SemanticChange {
        SemanticChange {
            entity_id: format!("f.rs::function::{name}"),
            change_type: "modified".into(),
            entity_type: "function".into(),
            entity_name: name.into(),
            start_line: 1,
            end_line: 2,
            old_start_line: None,
            old_end_line: None,
            old_entity_name: None,
            file_path: "f.rs".into(),
            old_file_path: None,
            structural_change: structural,
        }
    }

    fn diff_result(changes: Vec<SemanticChange>) -> SemanticDiffResult {
        SemanticDiffResult {
            engine_version: "0.23.1".into(),
            summary: Default::default(),
            changes,
            binary_changes: vec![],
        }
    }

    #[test]
    fn summary_prioritizes_structural_over_cosmetic() {
        let result = diff_result(vec![change(Some(false), "fmt_only"), change(Some(true), "logic")]);
        let summary = build_diff_summary(&result, DIFF_SUMMARY_BUDGET);
        let logic_pos = summary.find("logic").unwrap();
        let fmt_pos = summary.find("fmt_only").unwrap();
        assert!(logic_pos < fmt_pos);
        assert!(summary.contains("(structural)"));
        assert!(summary.contains("(cosmetic)"));
        assert!(summary.contains("f.rs: modified function logic"));
    }

    #[test]
    fn summary_respects_char_budget_and_marks_truncation() {
        let result = diff_result(
            (0..50)
                .map(|i| change(Some(true), &format!("entity_with_a_long_name_{i}")))
                .collect(),
        );
        let summary = build_diff_summary(&result, 200);
        assert!(summary.len() <= 240);
        assert!(summary.contains("semantic summary truncated"));
    }

    #[test]
    fn summary_empty_for_empty_diff() {
        assert_eq!(build_diff_summary(&diff_result(vec![]), 6000), "");
    }
}
