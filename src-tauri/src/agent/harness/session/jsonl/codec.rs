//! JSONL v4 行编解码:对齐 `packages/agent/src/harness/session/jsonl/codec.ts`。
//!
//! 头与变更行各占一行、以 `\n` 结尾;字段名与 TS 完全兼容(camelCase,entry/
//! record 载荷平铺在 `{kind: ...}` 对象中)。载荷结构校验由 session 层的 serde
//! 反序列化承担(蓝本的手工 require* 检查在等价程度上由类型化解析覆盖)。

use serde_json::Value;

use super::errors::{JsonlDecodeError, JsonlDecodeErrorKind};
use super::types::JsonlV4Header;
use crate::agent::harness::session::state::{session_mutation_from_value, SessionMutation};
use crate::agent::harness::types::{err, ok, Result};

/// 解析 v4 头行(不带换行;对齐 TS `parseHeader`)。
pub fn parse_header(line: &str) -> Result<JsonlV4Header, JsonlDecodeError> {
    decode_header(line)
}

fn decode_header(line: &str) -> Result<JsonlV4Header, JsonlDecodeError> {
    let value = parse_object(line)?;
    if value.get("kind").and_then(Value::as_str) != Some("header") {
        return Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            "is not a header",
        ));
    }
    if value.get("version").and_then(Value::as_i64) != Some(4) {
        return Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            "has unsupported session version",
        ));
    }
    let parent_session_id = match value.get("parentSessionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => {
            return Err(JsonlDecodeError::new(
                JsonlDecodeErrorKind::Schema,
                "has invalid parentSessionId",
            ))
        }
    };
    let legacy_parent_session_path = match value.get("legacyParentSessionPath") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) => Some(path.clone()),
        Some(_) => {
            return Err(JsonlDecodeError::new(
                JsonlDecodeErrorKind::Schema,
                "has invalid legacyParentSessionPath",
            ))
        }
    };
    if parent_session_id.is_some() && legacy_parent_session_path.is_some() {
        return Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            "has both parentSessionId and legacyParentSessionPath",
        ));
    }
    let metadata = match value.get("metadata") {
        None | Some(Value::Null) => None,
        Some(metadata @ Value::Object(_)) => metadata.as_object().cloned(),
        Some(_) => {
            return Err(JsonlDecodeError::new(
                JsonlDecodeErrorKind::Schema,
                "has invalid metadata",
            ))
        }
    };
    Ok(JsonlV4Header {
        kind: "header".to_string(),
        version: 4,
        id: require_string(&value, "id")?,
        created_at: require_timestamp(&value)?,
        cwd: require_string(&value, "cwd")?,
        parent_session_id,
        legacy_parent_session_path,
        metadata,
    })
}

/// 编码头行为带 `\n` 结尾的单行(对齐 TS `encodeHeader`)。
pub fn encode_header(header: &JsonlV4Header) -> String {
    format!(
        "{}\n",
        serde_json::to_string(header).expect("header serializes")
    )
}

/// 头 + 文件字段 → 会话元数据(对齐 TS `metadataFromHeader`)。
pub fn metadata_from_header(
    header: &JsonlV4Header,
    path: &str,
    modified_at: f64,
) -> super::types::JsonlSessionMetadata {
    super::types::JsonlSessionMetadata {
        id: header.id.clone(),
        created_at: header.created_at,
        cwd: header.cwd.clone(),
        path: path.to_string(),
        modified_at,
        source_format: 4,
        parent_session_id: header.parent_session_id.clone(),
        legacy_parent_session_path: header.legacy_parent_session_path.clone(),
        metadata: header.metadata.clone(),
    }
}

/// 解析变更行(不带换行;对齐 TS `parseMutation`)。
pub fn parse_mutation(line: &str) -> Result<SessionMutation, JsonlDecodeError> {
    let value = parse_object(line)?;
    // seq 校验与蓝本一致:安全正整数。
    let seq = value
        .get("seq")
        .and_then(Value::as_i64)
        .ok_or_else(|| JsonlDecodeError::new(JsonlDecodeErrorKind::Schema, "has invalid seq"))?;
    if seq <= 0 {
        return Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            "has invalid seq",
        ));
    }
    if matches!(
        value.get("kind").and_then(Value::as_str),
        Some("lane") | Some("fact")
    ) {
        // lane/fact 的 seq 已在 session_mutation_from_value 内校验;此处保持一致即可。
        let _ = seq;
    }
    session_mutation_from_value(Value::Object(value))
        .map_err(|error| JsonlDecodeError::new(JsonlDecodeErrorKind::Schema, error.message))
}

/// 编码变更为带 `\n` 结尾的单行(对齐 TS `encodeMutation`)。
pub fn encode_mutation(mutation: &SessionMutation) -> String {
    format!(
        "{}\n",
        serde_json::to_string(mutation).expect("session mutation serializes")
    )
}

fn parse_object(line: &str) -> Result<serde_json::Map<String, Value>, JsonlDecodeError> {
    let value: Value = serde_json::from_str(line).map_err(|error| {
        JsonlDecodeError::new(JsonlDecodeErrorKind::Syntax, "is not valid JSON")
            .with_cause(std::sync::Arc::new(error))
    })?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            "is not a JSON object",
        )),
    }
}

fn require_string(
    value: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, JsonlDecodeError> {
    match value.get(field) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            format!("has invalid {field}"),
        )),
    }
}

fn require_timestamp(value: &serde_json::Map<String, Value>) -> Result<i64, JsonlDecodeError> {
    match value.get("createdAt").and_then(Value::as_i64) {
        Some(timestamp) if timestamp >= 0 => Ok(timestamp),
        _ => Err(JsonlDecodeError::new(
            JsonlDecodeErrorKind::Schema,
            "has invalid timestamp",
        )),
    }
}

// 保留 ok/err 引用(与蓝本 Result 辅助对齐;直接返回值以简化)。
#[allow(dead_code)]
fn _result_helpers() {
    let _ = ok::<u8, JsonlDecodeError>(1);
    let _ = err::<u8, JsonlDecodeError>(JsonlDecodeError::new(JsonlDecodeErrorKind::Syntax, "x"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness::session::state::SessionMutation;
    use crate::agent::harness::session::types::{CustomEntry, Entry};
    use serde_json::json;

    fn custom_entry(seq: i64, parent_id: Option<&str>) -> Entry {
        Entry::Custom(CustomEntry {
            id: "entry-1".into(),
            seq,
            parent_id: parent_id.map(str::to_string),
            timestamp: 100,
            custom_type: "note".into(),
            data: Some(json!({"text": "hello"})),
        })
    }

    #[test]
    fn header_round_trip_with_parent() {
        let header = JsonlV4Header {
            kind: "header".into(),
            version: 4,
            id: "session".into(),
            created_at: 1_700_000_000_000,
            cwd: "/workspace/project".into(),
            parent_session_id: Some("parent".into()),
            legacy_parent_session_path: None,
            metadata: Some(
                json!({"owner": "agent", "nested": {"enabled": true}, "values": [1, null, "two"]})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        };
        let encoded = encode_header(&header);
        assert!(encoded.ends_with('\n'));
        let parsed = parse_header(encoded.trim_end()).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn header_round_trip_legacy_parent() {
        let header = JsonlV4Header::new("legacy-child", 1_700_000_000_001, "/workspace/project")
            .clone_with_legacy("/sessions/missing-parent.jsonl");
        let parsed = parse_header(&encode_header(&header)).unwrap();
        assert_eq!(parsed, header);
        assert!(parsed.parent_session_id.is_none());
        assert_eq!(
            parsed.legacy_parent_session_path.as_deref(),
            Some("/sessions/missing-parent.jsonl")
        );
    }

    #[test]
    fn header_rejects_both_parents_and_bad_version() {
        let line = r#"{"kind":"header","version":4,"id":"s","createdAt":1,"cwd":"/w","parentSessionId":"p","legacyParentSessionPath":"/x"}"#;
        let error = parse_header(line).unwrap_err();
        assert_eq!(error.kind, JsonlDecodeErrorKind::Schema);
        let line = r#"{"kind":"header","version":3,"id":"s","createdAt":1,"cwd":"/w"}"#;
        assert_eq!(
            parse_header(line).unwrap_err().kind,
            JsonlDecodeErrorKind::Schema
        );
    }

    #[test]
    fn metadata_projection_matches_ts() {
        let header = JsonlV4Header {
            kind: "header".into(),
            version: 4,
            id: "session".into(),
            created_at: 1_700_000_000_000,
            cwd: "/workspace/project".into(),
            parent_session_id: None,
            legacy_parent_session_path: Some("/sessions/missing-parent.jsonl".into()),
            metadata: Some(json!({"owner": "agent"}).as_object().cloned().unwrap()),
        };
        let metadata =
            metadata_from_header(&header, "/sessions/session.jsonl", 1_700_000_000_100.0);
        assert_eq!(metadata.id, "session");
        assert_eq!(metadata.path, "/sessions/session.jsonl");
        assert_eq!(metadata.modified_at, 1_700_000_000_100.0);
        assert_eq!(metadata.source_format, 4);
        assert_eq!(
            metadata.legacy_parent_session_path.as_deref(),
            Some("/sessions/missing-parent.jsonl")
        );
    }

    #[test]
    fn mutation_lines_round_trip() {
        let entry_mutation = SessionMutation::Entry {
            lane: Some("main".into()),
            entry: custom_entry(1, None),
        };
        let encoded = encode_mutation(&entry_mutation);
        assert!(encoded.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(encoded.trim_end()).unwrap();
        assert_eq!(value["kind"], "entry");
        assert_eq!(value["lane"], "main");
        assert_eq!(value["type"], "custom");
        assert_eq!(value["customType"], "note");
        assert_eq!(value["seq"], 1);
        assert_eq!(value["parentId"], Value::Null);
        let parsed = parse_mutation(encoded.trim_end()).unwrap();
        assert_eq!(parsed, entry_mutation);

        // 无 lane 的导入条目。
        let imported = SessionMutation::Entry {
            lane: None,
            entry: custom_entry(2, Some("entry-1")),
        };
        let parsed = parse_mutation(&encode_mutation(&imported).trim_end()).unwrap();
        assert_eq!(parsed, imported);

        let lane = SessionMutation::Lane {
            seq: 3,
            lane: "side".into(),
            leaf_id: Some("entry-1".into()),
        };
        let parsed = parse_mutation(&encode_mutation(&lane).trim_end()).unwrap();
        assert_eq!(parsed, lane);

        let fact = SessionMutation::Fact {
            seq: 4,
            fact: crate::agent::harness::session::types::SessionFact::Label {
                target_id: "entry-1".into(),
                label: Some("keep".into()),
            },
        };
        let value: serde_json::Value =
            serde_json::from_str(encode_mutation(&fact).trim_end()).unwrap();
        assert_eq!(value["kind"], "fact");
        assert_eq!(value["fact"], "label");
        assert_eq!(value["targetId"], "entry-1");
        let parsed = parse_mutation(encode_mutation(&fact).trim_end()).unwrap();
        assert_eq!(parsed, fact);
    }

    #[test]
    fn mutation_lines_syntax_and_schema_errors() {
        let syntax = parse_mutation("{").unwrap_err();
        assert_eq!(syntax.kind, JsonlDecodeErrorKind::Syntax);
        let schema = parse_mutation(r#"{"kind": "unknown", "seq": 1}"#).unwrap_err();
        assert_eq!(schema.kind, JsonlDecodeErrorKind::Schema);
    }

    #[test]
    fn record_lines_round_trip() {
        let record = SessionMutation::Record {
            record: crate::agent::harness::session::types::LaneRecord::OperationStarted(
                crate::agent::harness::session::types::OperationStartedRecord {
                    id: "op-1".into(),
                    seq: 5,
                    lane: "main".into(),
                    timestamp: 111,
                    source_leaf_id: None,
                    intent: crate::agent::harness::session::types::OperationIntent::Run {
                        original_prompt: vec![],
                        initial_messages: vec![],
                        system_prompt_override: None,
                        resume_data: None,
                    },
                },
            ),
        };
        let value: serde_json::Value =
            serde_json::from_str(encode_mutation(&record).trim_end()).unwrap();
        assert_eq!(value["kind"], "record");
        assert_eq!(value["type"], "operation_started");
        assert_eq!(value["intent"]["kind"], "run");
        let parsed = parse_mutation(encode_mutation(&record).trim_end()).unwrap();
        assert_eq!(parsed, record);
    }
}

impl JsonlV4Header {
    fn clone_with_legacy(mut self, path: &str) -> Self {
        self.legacy_parent_session_path = Some(path.to_string());
        self
    }
}
