//! AI 语义上下文:工作区 diff 摘要 / 单实体 context(第 4B 期)。

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use similar::TextDiff;
use tauri::AppHandle;

use crate::commands::git::open_repo;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::path_util::clean_str;

use super::models::{
    SemanticContextEntry, SemanticContextOmitted, SemanticContextResult, SemanticDiffResult,
};
use super::parse::parse_stdout;
use super::process::{run_sem, run_sem_with_input, SemRunPolicy};
use super::{
    detect_version, output_error, resolve_workdir, validate_entity_token, validate_rel_file_path,
};

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
    // 转为公开 DTO 时丢弃 beforeContent/afterContent,实体全文不进 IPC
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

/// AI 提交信息使用的 sem 主数据源。covered_paths 只包含 sem 确实返回实体或二进制记录的文件。
pub(crate) struct SemanticCommitAnalysis {
    pub text: String,
    pub covered_paths: HashSet<String>,
}

/// 从 RepoMeow 按本次提交范围构造的 FileChange JSON 分析变更。使用 --stdin 是因为 sem
/// 原生 worktree diff 与 git 一样排除未跟踪文件，且不能表达提交对话框的文件子集。
pub(crate) async fn commit_input_analysis(
    app: &AppHandle,
    path: &str,
    input: &str,
) -> AppResult<SemanticCommitAnalysis> {
    let root = resolve_workdir(path)?;
    let args = vec![
        "diff".to_string(),
        "--stdin".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ];
    let output = run_sem_with_input(
        app,
        Some(&root),
        &args,
        SemRunPolicy::CONTEXT,
        None,
        input.as_bytes(),
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let envelope: SemCliEnvelope = parse_stdout(&output.stdout)?;
    Ok(render_commit_analysis(&envelope))
}

fn normalized_sem_path(path: &str) -> String {
    crate::path_util::to_forward_slash_str(path)
}

const UNIFIED_DIFF_CONTEXT_LINES: usize = 2;

fn change_range(change: &super::models::SemCliChange) -> Option<(usize, usize)> {
    if change.start_line > 0 && change.end_line >= change.start_line {
        Some((change.start_line, change.end_line))
    } else {
        change
            .old_start_line
            .zip(change.old_end_line)
            .filter(|(start, end)| *start > 0 && end >= start)
    }
}

fn has_modified_content(change: &super::models::SemCliChange) -> bool {
    matches!(
        (&change.before_content, &change.after_content),
        (Some(before), Some(after)) if before != after
    )
}

fn entity_unified_diff(change: &super::models::SemCliChange) -> Option<String> {
    let (Some(before), Some(after)) = (&change.before_content, &change.after_content) else {
        return None;
    };
    if before == after {
        return None;
    }
    let diff = TextDiff::from_lines(before, after);
    let mut unified = diff.unified_diff();
    unified
        .context_radius(UNIFIED_DIFF_CONTEXT_LINES)
        .header("before", "after")
        .missing_newline_hint(true);
    let rendered = unified.to_string();
    rendered.contains("@@").then_some(rendered)
}

fn range_label(change: &super::models::SemCliChange) -> String {
    let current = format!("{}-{}", change.start_line, change.end_line);
    match (change.old_start_line, change.old_end_line) {
        (Some(old_start), Some(old_end))
            if old_start != change.start_line || old_end != change.end_line =>
        {
            format!("old {old_start}-{old_end} -> {current}")
        }
        _ => current,
    }
}

fn render_change(change: &super::models::SemCliChange, path: &str, include_diff: bool) -> String {
    let structural = match change.structural_change {
        Some(true) => "structural",
        Some(false) => "cosmetic",
        None => "semantic",
    };
    let mut out = format!(
        "- `{path}:{}` {} {} `{}` ({structural})",
        range_label(change),
        change.change_type,
        change.entity_type,
        change.entity_name,
    );
    if include_diff {
        if let Some(diff) = entity_unified_diff(change) {
            out.push_str("\n```diff\n");
            out.push_str(diff.trim_end());
            out.push_str("\n```");
        }
    }
    out
}

fn duplicate_key(change: &super::models::SemCliChange, path: &str) -> String {
    format!(
        "{path}\0{}\0{}\0{}\0{}\0{}\0{}\0{:?}\0{:?}",
        change.entity_id,
        change.change_type,
        change.entity_type,
        change.entity_name,
        change.start_line,
        change.end_line,
        change.old_start_line,
        change.old_end_line,
    )
}

/// 所有唯一实体都保留元数据；同范围重复内容或嵌套实体只在最外层输出一次 diff，
/// 避免 class/impl 与其中 method/function 重复发送相同修改。
fn should_include_entity_diff(index: usize, changes: &[&super::models::SemCliChange]) -> bool {
    let current = changes[index];
    if !has_modified_content(current) {
        return false;
    }
    let Some((current_start, current_end)) = change_range(current) else {
        return true;
    };
    for (other_index, other) in changes.iter().enumerate() {
        if other_index == index || !has_modified_content(other) {
            continue;
        }
        let Some((other_start, other_end)) = change_range(other) else {
            continue;
        };
        let same_content = current.before_content == other.before_content
            && current.after_content == other.after_content;
        if same_content
            && other_start == current_start
            && other_end == current_end
            && other_index < index
        {
            return false;
        }
        let strictly_contains = other_start <= current_start
            && other_end >= current_end
            && (other_start < current_start || other_end > current_end);
        if strictly_contains {
            return false;
        }
    }
    true
}

fn render_commit_analysis(envelope: &SemCliEnvelope) -> SemanticCommitAnalysis {
    let mut covered_paths = HashSet::new();
    let mut seen = HashSet::new();
    let mut by_file: BTreeMap<String, Vec<&super::models::SemCliChange>> = BTreeMap::new();
    for change in &envelope.changes {
        let path = normalized_sem_path(&change.file_path);
        covered_paths.insert(path.clone());
        if let Some(old_path) = &change.old_file_path {
            covered_paths.insert(normalized_sem_path(old_path));
        }
        if seen.insert(duplicate_key(change, &path)) {
            by_file.entry(path).or_default().push(change);
        }
    }

    let mut sections = Vec::new();
    for (path, changes) in by_file {
        let entries = changes
            .iter()
            .enumerate()
            .map(|(index, change)| {
                render_change(change, &path, should_include_entity_diff(index, &changes))
            })
            .collect::<Vec<_>>();
        sections.push(format!("### {path}\n{}", entries.join("\n")));
    }

    let mut binary_by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for binary in &envelope.binary_changes {
        let path = normalized_sem_path(&binary.file_path);
        covered_paths.insert(path.clone());
        if let Some(old_path) = &binary.old_file_path {
            covered_paths.insert(normalized_sem_path(old_path));
        }
        binary_by_file
            .entry(path.clone())
            .or_default()
            .push(format!(
                "- `{path}` binary {} ({})",
                binary.file_status, binary.change_type
            ));
    }
    for (path, entries) in binary_by_file {
        sections.push(format!("### {path}\n{}", entries.join("\n")));
    }
    sections.sort();

    SemanticCommitAnalysis {
        text: sections.join("\n\n"),
        covered_paths,
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

    fn raw_change(name: &str, path: &str, entity_type: &str) -> super::super::models::SemCliChange {
        super::super::models::SemCliChange {
            entity_id: format!("{path}::function::{name}"),
            change_type: "modified".into(),
            entity_type: entity_type.into(),
            entity_name: name.into(),
            start_line: 1,
            end_line: 2,
            old_start_line: Some(1),
            old_end_line: Some(2),
            old_entity_name: None,
            file_path: path.into(),
            old_file_path: None,
            structural_change: Some(true),
            before_content: Some("fn x() { old(); }".into()),
            after_content: Some("fn x() { new(); }".into()),
        }
    }

    #[test]
    fn commit_analysis_has_no_entity_count_limit() {
        let envelope = SemCliEnvelope {
            summary: Default::default(),
            changes: (0..250)
                .map(|index| raw_change(&format!("entity_{index}"), "src/main.rs", "function"))
                .collect(),
            binary_changes: vec![],
        };
        let analysis = render_commit_analysis(&envelope);
        assert!(analysis.text.contains("entity_249"));
        assert!(!analysis.text.contains("semantic summary truncated"));
        assert!(analysis.covered_paths.contains("src/main.rs"));
    }

    #[test]
    fn chunk_and_binary_results_count_as_covered() {
        let envelope = SemCliEnvelope {
            summary: Default::default(),
            changes: vec![raw_change("lines 1-2", "notes.txt", "chunk")],
            binary_changes: vec![super::super::models::SemanticBinaryChange {
                change_type: "binary".into(),
                file_path: "assets/logo.png".into(),
                old_file_path: None,
                file_status: "added".into(),
            }],
        };
        let analysis = render_commit_analysis(&envelope);
        assert!(analysis.covered_paths.contains("notes.txt"));
        assert!(analysis.covered_paths.contains("assets/logo.png"));
        assert!(analysis.text.contains("binary added"));
    }
}
