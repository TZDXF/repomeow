//! 影响分析:impact(第 3 期)。

use serde::Deserialize;

use crate::error::{AppError, AppResult, ErrorCode};

use super::models::{SemanticEntityRef, SemanticImpactResult, SemanticImpactedEntity};
use super::parse::{entity_ref, parse_stdout, truncate_to};
use super::process::{run_sem, SemLauncher, SemRunPolicy};
use super::{
    detect_version, output_error, resolve_workdir, validate_entity_token, validate_rel_file_path,
};

/// dependencies / dependents / tests 每类硬上限
const IMPACT_GROUP_LIMIT: usize = 1_000;
/// 传递影响节点硬上限
const IMPACT_AFFECTED_LIMIT: usize = 2_000;

// ── sem 0.23.1 原始 JSON(camelCase;未知字段默认忽略)──────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawImpactEntity {
    #[serde(rename = "entityId")]
    entity_id: String,
    file: String,
    #[serde(default)]
    lines: [usize; 2],
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawImpactedEntity {
    #[serde(flatten)]
    entity: RawImpactEntity,
    #[serde(default)]
    depth: usize,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawImpactSection {
    #[serde(default)]
    entities: Vec<RawImpactedEntity>,
    #[serde(default)]
    total: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawImpactOutput {
    entity: RawImpactEntity,
    #[serde(default)]
    dependencies: Vec<RawImpactEntity>,
    #[serde(default)]
    dependents: Vec<RawImpactEntity>,
    #[serde(default)]
    impact: RawImpactSection,
    #[serde(default)]
    tests: Vec<RawImpactEntity>,
}

impl From<RawImpactEntity> for SemanticEntityRef {
    fn from(raw: RawImpactEntity) -> Self {
        entity_ref(
            Some(raw.entity_id),
            raw.name,
            raw.entity_type,
            raw.file,
            raw.lines[0],
            raw.lines[1],
        )
    }
}

/// stderr 中的「实体不存在」判定(sem 文案:`error: Entity 'x' not found`)。
fn is_entity_not_found(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("not found")
}

/// 校验影响深度:缺省 2,只接受 1..=5(不向 UI 开放 0 = unlimited)。
fn validate_depth(depth: Option<usize>) -> AppResult<usize> {
    let depth = depth.unwrap_or(2);
    if !(1..=5).contains(&depth) {
        return Err(AppError::coded(
            ErrorCode::SemanticToolFailed,
            format!("depth out of range 1..=5: {depth}"),
        ));
    }
    Ok(depth)
}

pub(super) async fn entity_impact_impl(
    launcher: SemLauncher,
    path: String,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    depth: Option<usize>,
    request_id: Option<String>,
) -> AppResult<SemanticImpactResult> {
    let root = resolve_workdir(&path)?;
    let depth = validate_depth(depth)?;

    let mut args = vec!["impact".to_string()];
    // entityId 优先,缺省回退位置参数实体名
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
    args.push("--depth".to_string());
    args.push(depth.to_string());
    args.push("--json".to_string());

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

    let raw: RawImpactOutput = parse_stdout(&output.stdout)?;
    let mut truncated = false;

    let mut dependencies: Vec<SemanticEntityRef> =
        raw.dependencies.into_iter().map(Into::into).collect();
    truncated |= truncate_to(&mut dependencies, IMPACT_GROUP_LIMIT);
    let mut dependents: Vec<SemanticEntityRef> =
        raw.dependents.into_iter().map(Into::into).collect();
    truncated |= truncate_to(&mut dependents, IMPACT_GROUP_LIMIT);
    let mut tests: Vec<SemanticEntityRef> = raw.tests.into_iter().map(Into::into).collect();
    truncated |= truncate_to(&mut tests, IMPACT_GROUP_LIMIT);

    let total = raw.impact.total;
    let mut affected: Vec<SemanticImpactedEntity> = raw
        .impact
        .entities
        .into_iter()
        .map(|item| SemanticImpactedEntity {
            entity: item.entity.into(),
            depth: item.depth,
        })
        .collect();
    truncated |= truncate_to(&mut affected, IMPACT_AFFECTED_LIMIT);
    // sem 的 total 是全量计数,比返回实体多即结果被截断
    if total > affected.len() {
        truncated = true;
    }

    Ok(SemanticImpactResult {
        engine_version: version,
        entity: raw.entity.into(),
        dependencies,
        dependents,
        affected,
        tests,
        total,
        depth,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_impact_output() {
        // 契约来自 sem 0.23.1 impact --json 真实输出(缩写)
        let raw = br#"{
          "entity":{"entityId":"src-tauri/src/time_util.rs::function::now_ts","file":"src-tauri/src/time_util.rs","lines":[4,6],"name":"now_ts","type":"function"},
          "dependencies":[],
          "dependents":[{"entityId":"src-tauri/src/commands/account.rs::function::now","file":"src-tauri/src/commands/account.rs","lines":[43,45],"name":"now","type":"function"}],
          "impact":{"depth":2,"entities":[{"depth":1,"entityId":"src-tauri/src/commands/account.rs::function::now","file":"src-tauri/src/commands/account.rs","lines":[43,45],"name":"now","type":"function"}],"total":1},
          "tests":[{"entityId":"src-tauri/src/commands/report/tests.rs::module::tests::exact_match","file":"src-tauri/src/commands/report/tests.rs","lines":[496,519],"name":"exact_match","type":"function"}]
        }"#;
        let parsed: RawImpactOutput = parse_stdout(raw).unwrap();
        let entity: SemanticEntityRef = parsed.entity.into();
        assert_eq!(
            entity.entity_id.as_deref(),
            Some("src-tauri/src/time_util.rs::function::now_ts")
        );
        assert_eq!((entity.start_line, entity.end_line), (4, 6));
        assert_eq!(parsed.dependents.len(), 1);
        let impacted: Vec<SemanticImpactedEntity> = parsed
            .impact
            .entities
            .into_iter()
            .map(|item| SemanticImpactedEntity {
                entity: item.entity.into(),
                depth: item.depth,
            })
            .collect();
        assert_eq!(impacted[0].depth, 1);
        assert_eq!(parsed.tests.len(), 1);
    }

    #[test]
    fn tolerates_missing_optional_sections() {
        let raw = br#"{"entity":{"entityId":"f::function::a","file":"f","lines":[1,2],"name":"a","type":"function"}}"#;
        let parsed: RawImpactOutput = parse_stdout(raw).unwrap();
        assert!(parsed.dependencies.is_empty());
        assert!(parsed.impact.entities.is_empty());
        assert_eq!(parsed.impact.total, 0);
        // entity 缺 entityId 视为契约破坏
        assert!(parse_stdout::<RawImpactOutput>(
            br#"{"entity":{"file":"f","name":"a","type":"function"}}"#
        )
        .is_err());
    }

    #[test]
    fn depth_is_limited_to_one_through_five() {
        assert_eq!(validate_depth(None).unwrap(), 2);
        assert_eq!(validate_depth(Some(1)).unwrap(), 1);
        assert_eq!(validate_depth(Some(5)).unwrap(), 5);
        assert!(validate_depth(Some(0)).is_err());
        assert!(validate_depth(Some(6)).is_err());
    }

    #[test]
    fn entity_not_found_detected_from_stderr() {
        assert!(is_entity_not_found(b"error: Entity 'zzz' not found"));
        assert!(!is_entity_not_found(b"error: index corrupt"));
        assert!(!is_entity_not_found(b""));
    }

    #[test]
    fn total_beyond_returned_entities_marks_truncated() {
        // affected 只有 1 条而 total=66 时必须标记截断
        let mut affected: Vec<SemanticImpactedEntity> = vec![SemanticImpactedEntity {
            entity: entity_ref(
                Some("f::function::a".into()),
                "a".into(),
                "function".into(),
                "f".into(),
                1,
                2,
            ),
            depth: 1,
        }];
        let mut truncated = truncate_to(&mut affected, IMPACT_AFFECTED_LIMIT);
        let total = 66usize;
        if total > affected.len() {
            truncated = true;
        }
        assert!(truncated);
    }
}
