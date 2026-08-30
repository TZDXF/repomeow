//! 语义导航:entities / find / callers / refs(第 2 期)。

use serde::Deserialize;
use tauri::AppHandle;

use crate::error::{AppError, AppResult, ErrorCode};

use super::models::{
    SemanticEntityRef, SemanticFileEntitiesResult, SemanticFileEntity, SemanticFindResult,
    SemanticRelationGroup, SemanticRelationResult,
};
use super::parse::{build_entity_id, detects_path_scheme, entity_ref, parse_stdout, truncate_to};
use super::process::{run_sem, SemRunPolicy};
use super::{
    detect_version, output_error, resolve_workdir, validate_entity_token, validate_query,
    validate_rel_file_path,
};

/// 单文件实体硬上限
const ENTITIES_LIMIT: usize = 2_000;
/// find 结果硬上限
const FIND_LIMIT: usize = 100;
/// 每组 callers/refs 关系项硬上限
const RELATION_LIMIT: usize = 500;

/// callers / refs 关系查询类型。
pub(super) enum RelationKind {
    Callers,
    Refs,
}

impl RelationKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Callers => "callers",
            Self::Refs => "refs",
        }
    }
}

// ── sem 0.23.1 原始 JSON(snake_case;未知字段默认忽略)─────────────────

#[derive(Debug, Deserialize)]
struct RawFileEntity {
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    #[serde(default)]
    start_line: usize,
    #[serde(default)]
    end_line: usize,
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFoundEntity {
    id: String,
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    file: String,
    #[serde(default)]
    start_line: usize,
    #[serde(default)]
    end_line: usize,
}

#[derive(Debug, Deserialize)]
struct RawRelatedEntity {
    id: String,
    name: String,
    #[serde(rename = "type")]
    entity_type: String,
    file: String,
    #[serde(default)]
    start_line: usize,
    #[serde(default)]
    end_line: usize,
}

#[derive(Debug, Deserialize)]
struct RawRelationGroup {
    entity: RawRelatedEntity,
    #[serde(default)]
    related: Vec<RawRelatedEntity>,
}

impl From<RawRelatedEntity> for SemanticEntityRef {
    fn from(raw: RawRelatedEntity) -> Self {
        entity_ref(
            Some(raw.id),
            raw.name,
            raw.entity_type,
            raw.file,
            raw.start_line,
            raw.end_line,
        )
    }
}

/// 组装实体定位参数:entityId 优先,缺省回退实体名(位置参数),二者皆空报错。
fn entity_query_args(
    entity_id: Option<String>,
    entity_name: Option<String>,
) -> AppResult<Vec<String>> {
    if let Some(id) = entity_id {
        return Ok(vec![validate_entity_token(&id)?]);
    }
    if let Some(name) = entity_name {
        return Ok(vec![validate_entity_token(&name)?]);
    }
    Err(AppError::coded(
        ErrorCode::SemanticToolFailed,
        "entity_id/entity_name both empty".to_string(),
    ))
}

/// 可选的 --file 消歧参数。
fn entity_file_args(root: &std::path::Path, file_path: Option<String>) -> AppResult<Vec<String>> {
    match file_path {
        Some(file) => Ok(vec![
            "--file".to_string(),
            validate_rel_file_path(root, &file)?,
        ]),
        None => Ok(Vec::new()),
    }
}

pub(super) async fn file_entities_impl(
    app: AppHandle,
    path: String,
    file_path: String,
    request_id: Option<String>,
) -> AppResult<SemanticFileEntitiesResult> {
    let root = resolve_workdir(&path)?;
    let file = validate_rel_file_path(&root, &file_path)?;
    let version = detect_version(&app).await?;
    let args = vec!["entities".to_string(), file.clone(), "--json".to_string()];
    let output = run_sem(
        &app,
        Some(&root),
        &args,
        SemRunPolicy::NAV,
        request_id.as_deref(),
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let raw: Vec<RawFileEntity> = parse_stdout(&output.stdout)?;
    // JSON/YAML 等数据文件的 parent_id 是 `<file>::/<path>` 路径寻址,与代码文件的
    // `<file>::<type>::<name>` 方案不同,先按整个列表探测再构造 ID,父子才能对上
    let path_scheme = detects_path_scheme(&file, raw.iter().map(|e| e.parent_id.as_deref()));
    let mut entities: Vec<SemanticFileEntity> = raw
        .into_iter()
        .map(|item| {
            let id = build_entity_id(
                &file,
                &item.entity_type,
                &item.name,
                item.parent_id.as_deref(),
                path_scheme,
            );
            SemanticFileEntity {
                entity: entity_ref(
                    Some(id),
                    item.name,
                    item.entity_type,
                    file.clone(),
                    item.start_line,
                    item.end_line,
                ),
                parent_id: item.parent_id,
            }
        })
        .collect();
    let truncated = truncate_to(&mut entities, ENTITIES_LIMIT);
    Ok(SemanticFileEntitiesResult {
        engine_version: version,
        file_path: file,
        entities,
        truncated,
    })
}

pub(super) async fn find_entities_impl(
    app: AppHandle,
    path: String,
    query: String,
    request_id: Option<String>,
) -> AppResult<SemanticFindResult> {
    let root = resolve_workdir(&path)?;
    let query = validate_query(&query)?;
    let version = detect_version(&app).await?;
    let args = vec!["find".to_string(), query.clone(), "--json".to_string()];
    let output = run_sem(
        &app,
        Some(&root),
        &args,
        SemRunPolicy::NAV,
        request_id.as_deref(),
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let raw: Vec<RawFoundEntity> = parse_stdout(&output.stdout)?;
    let mut results: Vec<SemanticEntityRef> = raw
        .into_iter()
        .map(|item| {
            entity_ref(
                Some(item.id),
                item.name,
                item.entity_type,
                item.file,
                item.start_line,
                item.end_line,
            )
        })
        .collect();
    let truncated = truncate_to(&mut results, FIND_LIMIT);
    Ok(SemanticFindResult {
        engine_version: version,
        query,
        results,
        truncated,
    })
}

pub(super) async fn entity_relation_impl(
    app: AppHandle,
    path: String,
    kind: RelationKind,
    entity_id: Option<String>,
    entity_name: Option<String>,
    file_path: Option<String>,
    request_id: Option<String>,
) -> AppResult<SemanticRelationResult> {
    let root = resolve_workdir(&path)?;
    let mut args = vec![kind.as_str().to_string()];
    args.extend(entity_query_args(entity_id, entity_name)?);
    args.extend(entity_file_args(&root, file_path)?);
    args.push("--json".to_string());
    let version = detect_version(&app).await?;
    let output = run_sem(
        &app,
        Some(&root),
        &args,
        SemRunPolicy::NAV,
        request_id.as_deref(),
    )
    .await?;
    if output.code != Some(0) {
        return Err(output_error(output.code, &output.stderr));
    }
    let raw: Vec<RawRelationGroup> = parse_stdout(&output.stdout)?;
    let mut truncated = false;
    let groups: Vec<SemanticRelationGroup> = raw
        .into_iter()
        .map(|group| {
            let mut related: Vec<SemanticEntityRef> =
                group.related.into_iter().map(Into::into).collect();
            truncated |= truncate_to(&mut related, RELATION_LIMIT);
            SemanticRelationGroup {
                entity: group.entity.into(),
                related,
            }
        })
        .collect();
    Ok(SemanticRelationResult {
        engine_version: version,
        groups,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_entities_and_builds_nested_ids() {
        // 契约来自 sem 0.23.1 `entities src/lib/utils.ts --json` 的真实输出(缩写)
        let raw = br#"[
          {"name":"cn","type":"function","start_line":6,"end_line":8,"start_byte":156,"end_byte":235,"parent_id":null},
          {"name":"debounce","type":"function","start_line":21,"end_line":29,"parent_id":null},
          {"name":"run","type":"function","start_line":23,"end_line":26,"parent_id":"src/lib/utils.ts::function::debounce"}
        ]"#;
        let parsed: Vec<RawFileEntity> = parse_stdout(raw).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "cn");
        assert_eq!(parsed[0].parent_id, None);
        // 构造规则:根实体 <file>::<type>::<name>;嵌套 <parent_id>::<name>
        let nested = &parsed[2];
        let id = build_entity_id(
            "src/lib/utils.ts",
            &nested.entity_type,
            &nested.name,
            nested.parent_id.as_deref(),
            false,
        );
        assert_eq!(id, "src/lib/utils.ts::function::debounce::run");
        let root_id = build_entity_id("src/lib/utils.ts", "function", "cn", None, false);
        assert_eq!(root_id, "src/lib/utils.ts::function::cn");
    }

    #[test]
    fn file_entities_json_path_scheme_ids_link_parent_child() {
        // 契约来自 sem 0.23.1 `entities .oxlintrc.json --json` 的真实输出(缩写):
        // 无 id 字段,parent_id 为路径寻址;构造出的子级 parent 链必须能回指根实体 ID
        let raw = br#"[
          {"name":"categories","type":"object","start_line":4,"end_line":9,"parent_id":null},
          {"name":"correctness","type":"property","start_line":5,"end_line":5,"parent_id":".oxlintrc.json::/categories"}
        ]"#;
        let parsed: Vec<RawFileEntity> = parse_stdout(raw).unwrap();
        let file = ".oxlintrc.json";
        let path_scheme = detects_path_scheme(file, parsed.iter().map(|e| e.parent_id.as_deref()));
        assert!(path_scheme);
        let root = &parsed[0];
        let root_id = build_entity_id(file, &root.entity_type, &root.name, None, path_scheme);
        assert_eq!(root_id, ".oxlintrc.json::/categories");
        let child = &parsed[1];
        // 子级的 parent_id 与根实体构造 ID 一致,前端树才能挂上父子关系
        assert_eq!(child.parent_id.as_deref(), Some(root_id.as_str()));
        let child_id = build_entity_id(
            file,
            &child.entity_type,
            &child.name,
            child.parent_id.as_deref(),
            path_scheme,
        );
        assert_eq!(child_id, ".oxlintrc.json::/categories/correctness");
    }

    #[test]
    fn file_entities_tolerate_missing_optional_fields() {
        // 目录模式条目带 file、无 start_byte/end_byte;未知字段忽略
        let raw = br#"[{"name":"Provider","type":"type","start_line":4,"end_line":4,"parent_id":null,"file":"src/lib/accounts.ts","future":1}]"#;
        let parsed: Vec<RawFileEntity> = parse_stdout(raw).unwrap();
        assert_eq!(parsed[0].entity_type, "type");
        assert!(parse_stdout::<Vec<RawFileEntity>>(br#"[{"type":"function"}]"#).is_err());
    }

    #[test]
    fn parses_find_results() {
        let raw = br#"[{"id":"src/lib/utils.ts::function::debounce","name":"debounce","type":"function","file":"src/lib/utils.ts","start_line":21,"end_line":29}]"#;
        let parsed: Vec<RawFoundEntity> = parse_stdout(raw).unwrap();
        assert_eq!(parsed[0].id, "src/lib/utils.ts::function::debounce");
        // 缺 id 视为契约破坏
        assert!(parse_stdout::<Vec<RawFoundEntity>>(
            br#"[{"name":"x","type":"function","file":"f"}]"#
        )
        .is_err());
    }

    #[test]
    fn parses_relation_groups_and_empty_means_unknown() {
        let raw = br#"[{"entity":{"id":"f::function::a","name":"a","type":"function","file":"f","start_line":1,"end_line":2},"related":[{"id":"g::function::b","name":"b","type":"function","file":"g","start_line":3,"end_line":4}]}]"#;
        let parsed: Vec<RawRelationGroup> = parse_stdout(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].related.len(), 1);
        let entity: SemanticEntityRef = parsed.into_iter().next().unwrap().entity.into();
        assert_eq!(entity.entity_id.as_deref(), Some("f::function::a"));
        // 未知实体:空数组,exit 0
        let empty: Vec<RawRelationGroup> = parse_stdout(b"[]").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn relation_truncation_marks_result() {
        let mut related: Vec<SemanticEntityRef> = (0..=RELATION_LIMIT)
            .map(|i| {
                entity_ref(
                    Some(format!("id-{i}")),
                    "n".into(),
                    "function".into(),
                    "f".into(),
                    1,
                    1,
                )
            })
            .collect();
        assert!(truncate_to(&mut related, RELATION_LIMIT));
        assert_eq!(related.len(), RELATION_LIMIT);
    }

    #[test]
    fn entity_query_requires_id_or_name() {
        assert!(entity_query_args(None, None).is_err());
        assert!(entity_query_args(Some("  ".into()), None).is_err());
        assert!(entity_query_args(Some("a\0b".into()), None).is_err());
        assert_eq!(
            entity_query_args(Some("f::function::a".into()), None).unwrap(),
            vec!["f::function::a"]
        );
        assert_eq!(
            entity_query_args(None, Some("run".into())).unwrap(),
            vec!["run"]
        );
        // id 优先于 name
        assert_eq!(
            entity_query_args(Some("f::function::a".into()), Some("run".into())).unwrap(),
            vec!["f::function::a"]
        );
    }
}
