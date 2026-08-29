use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult, ErrorCode};

use super::models::SemanticEntityRef;

/// 解析 sem 的 stdout JSON;失败统一映射为 semantic_output_invalid。
pub(super) fn parse_stdout<T: DeserializeOwned>(bytes: &[u8]) -> AppResult<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| AppError::coded(ErrorCode::SemanticOutputInvalid, error.to_string()))
}

/// 集合硬上限:超出截断并返回 truncated 标记。
pub(super) fn truncate_to<T>(items: &mut Vec<T>, limit: usize) -> bool {
    if items.len() > limit {
        items.truncate(limit);
        true
    } else {
        false
    }
}

/// 按 sem 0.23.1 的 ID 规则构造实体 ID(已用真实 CLI 契约验证):
/// - 代码文件:根实体 `<file>::<type>::<name>`,嵌套 `<parent_id>::<name>`;
/// - 数据文件(JSON 等,parent_id 为 `<file>::/<path>` 路径寻址):
///   根实体 `<file>::/<name>`,嵌套 `<parent_id>/<name>`。
/// `path_scheme` 由调用方按整个实体列表探测(见 [`detects_path_scheme`])。
pub(super) fn build_entity_id(
    file_path: &str,
    entity_type: &str,
    name: &str,
    parent_id: Option<&str>,
    path_scheme: bool,
) -> String {
    if path_scheme {
        return match parent_id {
            Some(parent) if !parent.is_empty() => format!("{parent}/{name}"),
            _ => format!("{file_path}::/{name}"),
        };
    }
    match parent_id {
        Some(parent) if !parent.is_empty() => format!("{parent}::{name}"),
        _ => format!("{file_path}::{entity_type}::{name}"),
    }
}

/// 探测文件是否使用路径寻址 ID(JSON/YAML 等数据文件):
/// 任一实体的 parent_id 以 `<file>::/` 开头即整个文件都是该方案。
pub(super) fn detects_path_scheme<'a>(
    file_path: &str,
    parent_ids: impl Iterator<Item = Option<&'a str>>,
) -> bool {
    let prefix = format!("{file_path}::/");
    parent_ids.into_iter().flatten().any(|p| p.starts_with(&prefix))
}

pub(super) fn entity_ref(
    entity_id: Option<String>,
    name: String,
    entity_type: String,
    file_path: String,
    start_line: usize,
    end_line: usize,
) -> SemanticEntityRef {
    SemanticEntityRef {
        entity_id,
        name,
        entity_type,
        file_path,
        start_line,
        end_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_marks_only_when_exceeding_limit() {
        let mut items = vec![1, 2, 3];
        assert!(!truncate_to(&mut items, 3));
        assert_eq!(items, vec![1, 2, 3]);
        assert!(truncate_to(&mut items, 2));
        assert_eq!(items, vec![1, 2]);
    }

    #[test]
    fn entity_id_uses_parent_chain_when_nested() {
        assert_eq!(
            build_entity_id("src/a.ts", "function", "run", None, false),
            "src/a.ts::function::run"
        );
        assert_eq!(
            build_entity_id("src/a.ts", "function", "inner", Some("src/a.ts::function::run"), false),
            "src/a.ts::function::run::inner"
        );
        assert_eq!(
            build_entity_id("src/a.ts", "function", "run", Some(""), false),
            "src/a.ts::function::run"
        );
    }

    #[test]
    fn entity_id_path_scheme_matches_parent_addressing() {
        // 契约来自 sem 0.23.1 对 JSON 的真实输出:parent_id 为 `<file>::/<path>` 路径寻址
        assert!(detects_path_scheme(
            ".oxlintrc.json",
            [None, Some(".oxlintrc.json::/categories")].into_iter()
        ));
        assert!(!detects_path_scheme(
            "src/a.ts",
            [None, Some("src/a.ts::function::run")].into_iter()
        ));
        assert_eq!(
            build_entity_id(".oxlintrc.json", "object", "categories", None, true),
            ".oxlintrc.json::/categories"
        );
        assert_eq!(
            build_entity_id(
                ".oxlintrc.json",
                "property",
                "correctness",
                Some(".oxlintrc.json::/categories"),
                true,
            ),
            ".oxlintrc.json::/categories/correctness"
        );
    }
}
