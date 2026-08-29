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
/// 根实体为 `<file>::<type>::<name>`,嵌套实体为 `<parent_id>::<name>`。
pub(super) fn build_entity_id(
    file_path: &str,
    entity_type: &str,
    name: &str,
    parent_id: Option<&str>,
) -> String {
    match parent_id {
        Some(parent) if !parent.is_empty() => format!("{parent}::{name}"),
        _ => format!("{file_path}::{entity_type}::{name}"),
    }
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
            build_entity_id("src/a.ts", "function", "run", None),
            "src/a.ts::function::run"
        );
        assert_eq!(
            build_entity_id("src/a.ts", "function", "inner", Some("src/a.ts::function::run")),
            "src/a.ts::function::run::inner"
        );
        assert_eq!(
            build_entity_id("src/a.ts", "function", "run", Some("")),
            "src/a.ts::function::run"
        );
    }
}
