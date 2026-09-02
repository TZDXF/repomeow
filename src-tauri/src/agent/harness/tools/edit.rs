//! edit 工具:对齐 `packages/agent/src/harness/tools/edit.ts`。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::harness::tools::edit_diff::{
    apply_edits_to_normalized_content, detect_line_ending, generate_diff_string,
    generate_unified_patch, normalize_to_lf, restore_line_endings, strip_bom, Edit,
};
use crate::agent::harness::tools::file_mutation_queue::with_file_mutation_queue;
use crate::agent::harness::tools::path_utils::resolve_tool_path;
use crate::agent::harness::types::{ExecutionEnv, FileContent, FileError, SimpleError};
use crate::agent::types::{
    AbortSignal, AgentTool, AgentToolResult, PrepareArgumentsFn, ToolExecutionError,
};

/// edit 工具详情(对齐 TS `EditToolDetails`)。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditToolDetails {
    pub diff: String,
    pub patch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_changed_line: Option<usize>,
}

/// edit 工具参数(对齐 TS `EditToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditToolInput {
    pub path: String,
    pub edits: Vec<EditEntry>,
}

/// 单条替换(对齐 TS `edits[]` 元素)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditEntry {
    pub old_text: String,
    pub new_text: String,
}

/// 蓝本兼容参数准备:接受 `edits` 为 JSON 字符串/单对象与 legacy
/// `oldText`/`newText` 顶层字段(对齐 TS `prepareEditArguments`)。
pub fn prepare_edit_arguments() -> PrepareArgumentsFn {
    Arc::new(|input: Value| prepare_edit_arguments_impl(input))
}

fn is_single_edit_input(value: &Value) -> bool {
    match value {
        Value::Object(edit) => {
            edit.get("oldText").and_then(Value::as_str).is_some()
                && edit.get("newText").and_then(Value::as_str).is_some()
        }
        _ => false,
    }
}

fn prepare_edit_arguments_impl(input: Value) -> Value {
    let Value::Object(mut args) = input else {
        return input;
    };

    match args.get("edits") {
        Some(Value::String(text)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                match parsed {
                    Value::Array(items) => {
                        args.insert("edits".to_string(), Value::Array(items));
                    }
                    single if is_single_edit_input(&single) => {
                        args.insert("edits".to_string(), Value::Array(vec![single]));
                    }
                    _ => {}
                }
            }
        }
        Some(single) if is_single_edit_input(single) => {
            args.insert("edits".to_string(), Value::Array(vec![single.clone()]));
        }
        _ => {}
    }

    let legacy_old = args
        .get("oldText")
        .and_then(Value::as_str)
        .map(str::to_string);
    let legacy_new = args
        .get("newText")
        .and_then(Value::as_str)
        .map(str::to_string);
    let (Some(old_text), Some(new_text)) = (legacy_old, legacy_new) else {
        return Value::Object(args);
    };
    let mut edits = match args.get("edits") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    edits.push(json!({"oldText": old_text, "newText": new_text}));
    args.remove("oldText");
    args.remove("newText");
    args.insert("edits".to_string(), Value::Array(edits));
    Value::Object(args)
}

fn validate_edit_input(input: &EditToolInput) -> Result<Vec<Edit>, ToolExecutionError> {
    if input.edits.is_empty() {
        return Err(ToolExecutionError::from(SimpleError::new(
            "Edit tool input is invalid. edits must contain at least one replacement.",
        )));
    }
    Ok(input
        .edits
        .iter()
        .map(|edit| Edit {
            old_text: edit.old_text.clone(),
            new_text: edit.new_text.clone(),
        })
        .collect())
}

fn edit_access_error(path: &str, error: FileError) -> ToolExecutionError {
    ToolExecutionError::from(SimpleError::new(format!(
        "Could not edit file: {path}. Error code: {}.",
        error.code
    )))
}

/// 创建 edit 工具(唯一、非重叠的精确文本替换;返回 core AgentTool)。
pub fn create_edit_tool(env: Arc<dyn ExecutionEnv>) -> AgentTool {
    AgentTool {
        name: "edit".to_string(),
        label: "edit".to_string(),
        description: "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text for this targeted edit."
                            }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path", "edits"]
        }),
        execution_mode: None,
        prepare_arguments: Some(prepare_edit_arguments()),
        execute: Arc::new(move |_tool_call_id, params, signal, _on_update| {
            let env = env.clone();
            Box::pin(async move {
                let input: EditToolInput = serde_json::from_value(params)
                    .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))?;
                let edits = validate_edit_input(&input)?;
                let absolute_path = resolve_tool_path(env.as_ref(), &input.path, signal.clone())
                    .await
                    .map_err(|error| {
                        ToolExecutionError::from(SimpleError::new(error.to_string()))
                    })?;
                let queue_key = absolute_path.clone();
                let closure_path = absolute_path.clone();
                let result = with_file_mutation_queue(&env, &queue_key, {
                    let env = env.clone();
                    let input = input.clone();
                    let edits = edits.clone();
                    let signal: Option<AbortSignal> = signal.clone();
                    move || {
                        let env = env.clone();
                        let input = input.clone();
                        let edits = edits.clone();
                        let signal = signal.clone();
                        let absolute_path = closure_path.clone();
                        Box::pin(async move {
                        let aborted = || {
                            signal
                                .as_ref()
                                .map(|s| s.is_cancelled())
                                .unwrap_or(false)
                        };
                        if aborted() {
                            return Err(ToolExecutionError::from(SimpleError::new("Operation aborted")));
                        }
                        let info = env.file_info(absolute_path.clone()).await;
                        let info = match info {
                            Ok(info) => info,
                            Err(error) => return Err(edit_access_error(&input.path, error)),
                        };
                        if info.kind != crate::agent::harness::types::FileKind::File
                            && info.kind != crate::agent::harness::types::FileKind::Symlink
                        {
                            return Err(ToolExecutionError::from(SimpleError::new(format!(
                                "Could not edit file: {}. Path is not a file.",
                                input.path
                            ))));
                        }

                        let read_result = env.read_text_file(absolute_path.clone(), signal.clone()).await;
                        let read_result = match read_result {
                            Ok(content) => content,
                            Err(error) => return Err(edit_access_error(&input.path, error)),
                        };
                        if aborted() {
                            return Err(ToolExecutionError::from(SimpleError::new("Operation aborted")));
                        }

                        let (bom, content) = strip_bom(&read_result);
                        let original_ending = detect_line_ending(&content);
                        let normalized_content = normalize_to_lf(&content);
                        let applied = match apply_edits_to_normalized_content(&normalized_content, &edits, &input.path) {
                            Ok(applied) => applied,
                            Err(message) => {
                                return Err(ToolExecutionError::from(SimpleError::new(message)))
                            }
                        };
                        if aborted() {
                            return Err(ToolExecutionError::from(SimpleError::new("Operation aborted")));
                        }

                        let final_content = format!(
                            "{}{}",
                            bom,
                            restore_line_endings(&applied.new_content, original_ending)
                        );
                        let write_result = env
                            .write_file(absolute_path.clone(), FileContent::Text(final_content), signal.clone())
                            .await;
                        if let Err(error) = write_result {
                            return Err(edit_access_error(&input.path, error));
                        }
                        if aborted() {
                            return Err(ToolExecutionError::from(SimpleError::new("Operation aborted")));
                        }

                        let (diff, first_changed_line) =
                            generate_diff_string(&applied.base_content, &applied.new_content, 4);
                        Ok(AgentToolResult {
                            content: vec![crate::agent::types::TextOrImageContent::text(format!(
                                "Successfully replaced {} block(s) in {}.",
                                edits.len(),
                                input.path
                            ))],
                            details: serde_json::to_value(EditToolDetails {
                                diff,
                                patch: generate_unified_patch(&input.path, &applied.base_content, &applied.new_content, 4),
                                first_changed_line,
                            })
                            .unwrap_or(Value::Null),
                            ..Default::default()
                        })
                        })
                    }
                })
                .await;
                match result {
                    Ok(inner) => inner,
                    Err(error) => Err(ToolExecutionError::from(SimpleError::new(error.to_string()))),
                }
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_legacy_arguments() {
        let prepared = prepare_edit_arguments_impl(json!({
            "path": "/f.txt",
            "oldText": "a",
            "newText": "b"
        }));
        assert_eq!(prepared["edits"], json!([{"oldText": "a", "newText": "b"}]));
        assert!(prepared.get("oldText").is_none());

        let prepared = prepare_edit_arguments_impl(json!({
            "path": "/f.txt",
            "edits": "{\"oldText\":\"a\",\"newText\":\"b\"}"
        }));
        assert_eq!(prepared["edits"], json!([{"oldText": "a", "newText": "b"}]));

        let prepared = prepare_edit_arguments_impl(json!({
            "path": "/f.txt",
            "edits": {"oldText": "a", "newText": "b"}
        }));
        assert_eq!(prepared["edits"], json!([{"oldText": "a", "newText": "b"}]));

        // 数组保持原样。
        let prepared = prepare_edit_arguments_impl(json!({
            "path": "/f.txt",
            "edits": [{"oldText": "a", "newText": "b"}]
        }));
        assert_eq!(prepared["edits"], json!([{"oldText": "a", "newText": "b"}]));
    }
}
