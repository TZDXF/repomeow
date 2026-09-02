//! 工具参数 JSON Schema 校验:对齐 pi-ai 的 `validateToolArguments`
//! (packages/ai/src/utils/validation.ts)。
//!
//! 契约(与 agent-loop 的调用约定):校验失败返回 Err(消息面向 LLM 可读,
//! 含出错的 JSON Pointer 路径),成功返回(可能补齐默认值/类型强转后的)参数对象。
//!
//! 对齐行为:
//! 1. `normalizeOptionalNulls`:可选(非 required)属性上的 null,当子 schema
//!    拒绝 null 时直接删除;其余递归处理。
//! 2. `coerceWithJsonSchema`:字符串数字/布尔等按 schema 类型强转(allOf/anyOf/
//!    oneOf/properties/items/additionalProperties 递归),逐层应用。
//! 3. jsonschema crate 做最终校验;错误消息格式与 TS 一致:
//!    `  - <路径>: <消息>`,路径为 JSON Pointer 转点分(required 补属性名,
//!    空路径记 "root"),末尾附 `Received arguments:` 与原始参数 pretty JSON。
//!    TS 版消息含工具名(`Validation failed for tool "<name>"`),本函数签名
//!    不携带工具名,退化为 `Validation failed for tool arguments:`。

use std::collections::HashSet;

use serde_json::Value;

/// 校验工具调用参数是否符合工具的 JSON Schema。
/// `parameters` 为工具的 JSON Schema,`args` 为 LLM 输出的原始参数对象。
pub fn validate_tool_arguments(parameters: &Value, args: Value) -> Result<Value, String> {
    let original = args.clone();
    let mut coerced = args;
    normalize_optional_nulls(&mut coerced, parameters);
    let changed = coerce_with_json_schema(&mut coerced, parameters);

    let validator = match jsonschema::validator_for(parameters) {
        Ok(validator) => validator,
        // TS 侧 Compile 抛异常 → agent-loop 捕获为错误结果;这里等价返回 Err。
        Err(error) => return Err(format!("Invalid tool parameters schema: {error}")),
    };

    // TS:coercion 生效且双方非「对象/数组」时,校验失败退回未校验的原始 args。
    let both_containers = is_container(&coerced) && is_container(&original);
    if changed && !both_containers {
        if validator.is_valid(&coerced) {
            return Ok(coerced);
        }
        return Ok(original);
    }

    if validator.is_valid(&coerced) {
        return Ok(coerced);
    }

    Err(format_validation_error(&validator, &coerced, &original))
}

fn is_container(value: &Value) -> bool {
    value.is_object() || value.is_array()
}

// ── 错误消息(TS formatValidationPath) ────────────────────────────────

fn format_validation_error(
    validator: &jsonschema::Validator,
    coerced: &Value,
    original: &Value,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    for error in validator.iter_errors(coerced) {
        lines.push(format!("  - {}: {error}", format_validation_path(&error)));
    }
    let errors = if lines.is_empty() {
        "Unknown validation error".to_string()
    } else {
        lines.join("\n")
    };
    let received = serde_json::to_string_pretty(original).unwrap_or_default();
    format!("Validation failed for tool arguments:\n{errors}\n\nReceived arguments:\n{received}")
}

/// TS formatValidationPath:required 错误取父对象路径 + 属性名;
/// 其余取 instancePath(`/` 换 `.`,根记 "root")。
fn format_validation_path(error: &jsonschema::ValidationError<'_>) -> String {
    if let jsonschema::error::ValidationErrorKind::Required { property } = error.kind() {
        let property = property.as_str().unwrap_or_default().to_string();
        let base = pointer_to_dot_path(error.instance_path().as_str());
        return if base.is_empty() {
            property
        } else {
            format!("{base}.{property}")
        };
    }
    pointer_to_dot_path(error.instance_path().as_str())
}

/// JSON Pointer(`/a/0/b`)→ 点分路径(`a.0.b`);根为空串(非 required
/// 错误在调用方兜底为 "root")。
fn pointer_to_dot_path(pointer: &str) -> String {
    if pointer.is_empty() {
        return String::new();
    }
    pointer.trim_start_matches('/').replace('/', ".")
}

// ── 可选 null 清理(TS normalizeOptionalNulls) ─────────────────────────

fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    match value {
        Value::Array(items) => {
            match schema.get("items") {
                // tuple 形式逐位对齐
                Some(Value::Array(tuple)) => {
                    for (index, item) in items.iter_mut().enumerate() {
                        if let Some(item_schema) = tuple.get(index) {
                            normalize_optional_nulls(item, item_schema);
                        }
                    }
                }
                Some(item_schema) if item_schema.is_object() => {
                    for item in items.iter_mut() {
                        normalize_optional_nulls(item, item_schema);
                    }
                }
                _ => {}
            }
        }
        Value::Object(object) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                return;
            };
            let required: HashSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for (key, property_schema) in properties {
                if !object.contains_key(key) {
                    continue;
                }
                let is_null = object.get(key).is_some_and(Value::is_null);
                let not_required = !required.contains(key.as_str());
                let not_ref = property_schema
                    .get("$ref")
                    .and_then(Value::as_str)
                    .is_none();
                // TS:validator 存在且 Check(null) === false 才删除;编译失败保持原值
                let null_rejected = match jsonschema::validator_for(property_schema) {
                    Ok(sub) => !sub.is_valid(&Value::Null),
                    Err(_) => false,
                };
                if is_null && not_required && not_ref && null_rejected {
                    object.remove(key);
                } else if let Some(current) = object.get_mut(key) {
                    normalize_optional_nulls(current, property_schema);
                }
            }
        }
        _ => {}
    }
}

// ── 类型强转(TS coerceWithJsonSchema 系列) ────────────────────────────

/// 原地 coercion;返回是否发生了任何变更(对齐 TS `coerced !== args` 判断)。
fn coerce_with_json_schema(value: &mut Value, schema: &Value) -> bool {
    let mut changed = false;

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for nested in all_of {
            changed |= coerce_with_json_schema(value, nested);
        }
    }
    for combiner in ["anyOf", "oneOf"] {
        if let Some(schemas) = schema.get(combiner).and_then(Value::as_array) {
            changed |= coerce_with_union_schema(value, schemas);
        }
    }

    let schema_types = get_schema_types(schema);
    let matches_union =
        schema_types.len() > 1 && schema_types.iter().any(|t| matches_json_type(value, t));
    if !schema_types.is_empty() && !matches_union {
        for schema_type in &schema_types {
            if let Some(coerced) = coerce_primitive_by_type(value, schema_type) {
                *value = coerced;
                changed = true;
                break;
            }
        }
    }

    if schema_types.contains(&"object") && value.is_object() {
        changed |= apply_schema_object_coercion(value, schema);
    }
    if schema_types.contains(&"array") && value.is_array() {
        changed |= apply_schema_array_coercion(value, schema);
    }

    changed
}

fn get_schema_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

fn matches_json_type(value: &Value, json_type: &str) -> bool {
    match json_type {
        "number" => value.is_number(),
        "integer" => value
            .as_f64()
            .is_some_and(|n| n.fract() == 0.0 && n.is_finite()),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

/// TS coercePrimitiveByType 产出的数值:整数值收敛为 i64(对齐 JS Number 的
/// JSON 序列化,避免 serde_json 把 5 写成 5.0);超范围保留浮点。
fn number_value(n: f64) -> Value {
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0; // 2^53 - 1
    if n.is_finite() && n.fract() == 0.0 && n.abs() <= MAX_SAFE {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// 返回 Some(新值) 表示发生了类型强转(对齐 TS `candidate !== nextValue`)。
fn coerce_primitive_by_type(value: &Value, json_type: &str) -> Option<Value> {
    match json_type {
        "number" => match value {
            Value::Null => Some(Value::from(0)),
            Value::String(text) if !text.trim().is_empty() => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(number_value),
            Value::Bool(flag) => Some(Value::from(i64::from(*flag))),
            _ => None,
        },
        "integer" => match value {
            Value::Null => Some(Value::from(0)),
            Value::String(text) if !text.trim().is_empty() => text
                .trim()
                .parse::<i64>()
                .ok()
                .map(Value::from)
                .or_else(|| {
                    text.trim()
                        .parse::<f64>()
                        .ok()
                        .filter(|n| n.is_finite() && n.fract() == 0.0)
                        .map(number_value)
                }),
            Value::Bool(flag) => Some(Value::from(i64::from(*flag))),
            _ => None,
        },
        "boolean" => match value {
            Value::Null => Some(Value::Bool(false)),
            Value::String(text) => match text.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            Value::Number(number) => {
                if number.as_i64() == Some(1) {
                    Some(Value::Bool(true))
                } else if number.as_i64() == Some(0) {
                    Some(Value::Bool(false))
                } else {
                    None
                }
            }
            _ => None,
        },
        "string" => match value {
            Value::Null => Some(Value::String(String::new())),
            Value::Number(number) => Some(Value::String(number.to_string())),
            Value::Bool(flag) => Some(Value::String(flag.to_string())),
            _ => None,
        },
        "null" => match value {
            Value::String(text) if text.is_empty() => Some(Value::Null),
            Value::Number(number) if number.as_i64() == Some(0) => Some(Value::Null),
            Value::Bool(false) => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}

fn coerce_with_union_schema(value: &mut Value, schemas: &[Value]) -> bool {
    // 先看原值是否已被某个分支接受
    for schema in schemas {
        if let Ok(sub) = jsonschema::validator_for(schema) {
            if sub.is_valid(value) {
                return false;
            }
        }
    }
    // 再逐分支强转后校验
    for schema in schemas {
        let mut candidate = value.clone();
        coerce_with_json_schema(&mut candidate, schema);
        if let Ok(sub) = jsonschema::validator_for(schema) {
            if sub.is_valid(&candidate) {
                *value = candidate;
                return true;
            }
        }
    }
    false
}

fn apply_schema_object_coercion(value: &mut Value, schema: &Value) -> bool {
    let mut changed = false;
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (key, property_schema) in properties {
            if let Some(current) = value.get_mut(key) {
                changed |= coerce_with_json_schema(current, property_schema);
            }
        }
    }
    if let Some(additional) = schema.get("additionalProperties") {
        if additional.is_object() {
            let defined: HashSet<&str> = schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| properties.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let keys: Vec<String> = value
                .as_object()
                .map(|object| object.keys().cloned().collect())
                .unwrap_or_default();
            for key in keys {
                if defined.contains(key.as_str()) {
                    continue;
                }
                if let Some(current) = value.get_mut(&key) {
                    changed |= coerce_with_json_schema(current, additional);
                }
            }
        }
    }
    changed
}

fn apply_schema_array_coercion(value: &mut Value, schema: &Value) -> bool {
    let mut changed = false;
    let Some(items) = value.as_array_mut() else {
        return false;
    };
    match schema.get("items") {
        Some(Value::Array(tuple)) => {
            for (index, item) in items.iter_mut().enumerate() {
                if let Some(item_schema) = tuple.get(index) {
                    changed |= coerce_with_json_schema(item, item_schema);
                }
            }
        }
        Some(item_schema) if item_schema.is_object() => {
            for item in items.iter_mut() {
                changed |= coerce_with_json_schema(item, item_schema);
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_required_reports_property_path() {
        let schema = json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"],
        });
        let error = validate_tool_arguments(&schema, json!({})).unwrap_err();
        assert!(
            error.contains("- query: \"query\" is a required property"),
            "{error}"
        );
        assert!(
            error.starts_with("Validation failed for tool arguments:"),
            "{error}"
        );
        assert!(error.contains("Received arguments:"), "{error}");
        assert!(error.contains("{}"), "{error}");
    }

    #[test]
    fn missing_required_in_nested_object_carries_path() {
        let schema = json!({
            "type": "object",
            "properties": {
                "options": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"],
                }
            },
        });
        let error = validate_tool_arguments(&schema, json!({"options": {}})).unwrap_err();
        assert!(
            error.contains("- options.name: \"name\" is a required property"),
            "{error}"
        );
    }

    #[test]
    fn number_to_string_is_coerced_like_ts() {
        // TS coercePrimitiveByType("string"):number → String(value)
        let schema = json!({"type": "object", "properties": {"count": {"type": "string"}}});
        let args = validate_tool_arguments(&schema, json!({"count": 5})).unwrap();
        assert_eq!(args, json!({"count": "5"}));
    }

    #[test]
    fn incoercible_type_mismatch_reports_error_path() {
        // 对象无法强转成 string → 校验失败,错误带属性路径
        let schema = json!({"type": "object", "properties": {"count": {"type": "string"}}});
        let error =
            validate_tool_arguments(&schema, json!({"count": {"nested": true}})).unwrap_err();
        assert!(error.contains("- count: "), "{error}");
    }

    #[test]
    fn string_number_is_coerced_for_number_schema() {
        let schema = json!({"type": "object", "properties": {"limit": {"type": "number"}}});
        let args = validate_tool_arguments(&schema, json!({"limit": "5"})).unwrap();
        assert_eq!(args, json!({"limit": 5}));
        let args = validate_tool_arguments(&schema, json!({"limit": "5.5"})).unwrap();
        assert_eq!(args, json!({"limit": 5.5}));
    }

    #[test]
    fn string_integer_is_coerced_and_fractional_string_rejected() {
        let schema =
            json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]});
        let args = validate_tool_arguments(&schema, json!({"n": "42"})).unwrap();
        assert_eq!(args, json!({"n": 42}));
        assert!(validate_tool_arguments(&schema, json!({"n": "4.2"})).is_err());
    }

    #[test]
    fn string_boolean_is_coerced() {
        let schema = json!({"type": "object", "properties": {"flag": {"type": "boolean"}}});
        let args = validate_tool_arguments(&schema, json!({"flag": "true"})).unwrap();
        assert_eq!(args, json!({"flag": true}));
    }

    #[test]
    fn optional_null_is_dropped_when_schema_rejects_null() {
        let schema = json!({
            "type": "object",
            "properties": {
                "must": {"type": "string"},
                "opt": {"type": "string"},
            },
            "required": ["must"],
        });
        let args = validate_tool_arguments(&schema, json!({"must": "x", "opt": null})).unwrap();
        assert_eq!(args, json!({"must": "x"}));
    }

    #[test]
    fn required_null_is_coerced_to_empty_string() {
        // TS:required 属性不走 null 删除,coercion 将 null → ""(string schema)
        let schema = json!({
            "type": "object",
            "properties": {"opt": {"type": "string"}},
            "required": ["opt"],
        });
        let args = validate_tool_arguments(&schema, json!({"opt": null})).unwrap();
        assert_eq!(args, json!({"opt": ""}));
    }

    #[test]
    fn null_accepted_by_nullable_union_is_kept() {
        let schema = json!({
            "type": "object",
            "properties": {"opt": {"anyOf": [{"type": "string"}, {"type": "null"}]}},
        });
        let args = validate_tool_arguments(&schema, json!({"opt": null})).unwrap();
        assert_eq!(args, json!({"opt": null}));
    }

    #[test]
    fn nested_and_array_items_are_coerced() {
        let schema = json!({
            "type": "object",
            "properties": {
                "list": {
                    "type": "array",
                    "items": {"type": "integer"},
                }
            },
        });
        let args = validate_tool_arguments(&schema, json!({"list": ["1", "2"]})).unwrap();
        assert_eq!(args, json!({"list": [1, 2]}));
    }

    #[test]
    fn non_object_args_fail_object_schema() {
        let schema = json!({"type": "object"});
        assert!(validate_tool_arguments(&schema, json!("text")).is_err());
    }

    #[test]
    fn scalar_coercion_failure_errors() {
        // TS:coercion 无变化 → 直接校验原值,失败即抛
        let schema = json!({"type": "number"});
        assert!(validate_tool_arguments(&schema, json!("abc")).is_err());
    }

    #[test]
    fn scalar_coercion_success_returns_coerced() {
        // TS:coerced !== args 且双方非对象 → 校验 coerced,通过即返回
        let schema = json!({"type": "string"});
        let args = validate_tool_arguments(&schema, json!(5)).unwrap();
        assert_eq!(args, json!("5"));
    }

    #[test]
    fn invalid_schema_returns_schema_error() {
        let schema = json!({"type": 42});
        assert!(validate_tool_arguments(&schema, json!({}))
            .unwrap_err()
            .starts_with("Invalid tool parameters schema"));
    }

    #[test]
    fn additional_properties_schema_coercion_applies() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "additionalProperties": {"type": "number"},
        });
        let args = validate_tool_arguments(&schema, json!({"a": "3"})).unwrap();
        assert_eq!(args, json!({"a": 3}));
    }

    #[test]
    fn valid_args_pass_through_with_no_changes() {
        let schema = json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "line": {"type": "integer"}},
            "required": ["path"],
        });
        let args = validate_tool_arguments(&schema, json!({"path": "a.rs", "line": 3})).unwrap();
        assert_eq!(args, json!({"path": "a.rs", "line": 3}));
    }
}
