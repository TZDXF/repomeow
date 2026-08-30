//! read 工具:对齐 `packages/agent/src/harness/tools/read.ts`。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::harness::tools::image::{detect_supported_image_mime_type, encode_base64};
use crate::agent::harness::tools::path_utils::resolve_read_tool_path;
use crate::agent::harness::types::{
    ExecutionEnv, FileContent, SimpleError,
};
use crate::agent::harness::utils::truncate::{
    format_size, truncate_head, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncationResult,
};
use crate::agent::types::{AgentTool, TextOrImageContent, ToolExecutionError};

/// read 工具详情(对齐 TS `ReadToolDetails`)。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
}

/// read 工具选项(对齐 TS `ReadToolOptions`;注入式图片处理器暂不建模)。
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadToolOptions {
    pub auto_resize_images: bool,
}

/// read 工具参数(对齐 TS `ReadToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadToolInput {
    pub path: String,
    pub offset: Option<f64>,
    pub limit: Option<f64>,
}

fn read_tool_description() -> String {
    format!(
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). Images are sent as attachments. For text files, output is truncated to {DEFAULT_MAX_LINES} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
        DEFAULT_MAX_BYTES / 1024
    )
}

/// 创建 read 工具(构造时捕获 env;返回 core AgentTool)。
pub fn create_read_tool(env: Arc<dyn ExecutionEnv>, options: Option<ReadToolOptions>) -> AgentTool {
    let _ = options;
    AgentTool {
        name: "read".to_string(),
        label: "read".to_string(),
        description: read_tool_description(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative or absolute)"
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, params, signal, _on_update| {
            let env = env.clone();
            Box::pin(async move {
                let input: ReadToolInput = serde_json::from_value(params)
                    .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))?;
                let absolute_path =
                    resolve_read_tool_path(env.as_ref(), &input.path, signal.clone()).await;
                let bytes = crate::agent::harness::types::get_or_throw(
                    env.read_binary_file(absolute_path.clone(), signal.clone()).await,
                );
                if let Some(mime_type) = detect_supported_image_mime_type(&bytes) {
                    if mime_type == "image/bmp" {
                        return Ok(crate::agent::types::AgentToolResult {
                            content: vec![TextOrImageContent::text(
                                "Read image file [image/bmp]\n[Image omitted: configure an imageProcessor to convert BMP images.]",
                            )],
                            ..Default::default()
                        });
                    }
                    return Ok(crate::agent::types::AgentToolResult {
                        content: vec![
                            TextOrImageContent::text(format!("Read image file [{mime_type}]")),
                            TextOrImageContent::Image {
                                data: encode_base64(&bytes),
                                mime_type: mime_type.to_string(),
                            },
                        ],
                        ..Default::default()
                    });
                }

                let text_content = String::from_utf8_lossy(&bytes).to_string();
                let all_lines: Vec<&str> = text_content.split('\n').collect();
                let total_file_lines = all_lines.len();
                let start_line = input.offset.map(|offset| offset.max(1.0) as usize - 1).unwrap_or(0);
                let start_line_display = start_line + 1;
                if start_line >= all_lines.len() {
                    return Err(ToolExecutionError::from(SimpleError::new(format!(
                        "Offset {} is beyond end of file ({} lines total)",
                        input
                            .offset
                            .map(|offset| offset.to_string())
                            .unwrap_or_else(|| "undefined".to_string()),
                        all_lines.len()
                    ))));
                }

                let (selected_content, user_limited_lines): (String, Option<usize>) =
                    if let Some(limit) = input.limit {
                        let end_line = (start_line + limit.max(0.0) as usize).min(all_lines.len());
                        (
                            all_lines[start_line..end_line].join("\n"),
                            Some(end_line - start_line),
                        )
                    } else {
                        (all_lines[start_line..].join("\n"), None)
                    };

                let truncation = truncate_head(&selected_content, Default::default());
                let output_text;
                let mut details: Option<Value> = None;
                if truncation.first_line_exceeds_limit {
                    let first_line_size = format_size(all_lines[start_line].len());
                    output_text = format!(
                        "[Line {start_line_display} is {first_line_size}, exceeds {} limit. Use bash: sed -n '{start_line_display}p' {} | head -c {DEFAULT_MAX_BYTES}]",
                        format_size(DEFAULT_MAX_BYTES),
                        input.path
                    );
                    details = Some(
                        serde_json::to_value(ReadToolDetails {
                            truncation: Some(truncation),
                        })
                        .unwrap_or(Value::Null),
                    );
                } else if truncation.truncated {
                    let end_line_display = start_line_display + truncation.output_lines - 1;
                    let next_offset = end_line_display + 1;
                    let mut tail = truncation.content.clone();
                    if truncation.truncated_by == Some(crate::agent::harness::utils::truncate::TruncatedBy::Lines) {
                        tail.push_str(&format!(
                            "\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines}. Use offset={next_offset} to continue.]"
                        ));
                    } else {
                        tail.push_str(&format!(
                            "\n\n[Showing lines {start_line_display}-{end_line_display} of {total_file_lines} ({} limit). Use offset={next_offset} to continue.]",
                            format_size(DEFAULT_MAX_BYTES)
                        ));
                    }
                    output_text = tail;
                    details = Some(
                        serde_json::to_value(ReadToolDetails {
                            truncation: Some(truncation),
                        })
                        .unwrap_or(Value::Null),
                    );
                } else if let Some(user_limited_lines) = user_limited_lines {
                    if start_line + user_limited_lines < all_lines.len() {
                        let remaining = all_lines.len() - (start_line + user_limited_lines);
                        let next_offset = start_line + user_limited_lines + 1;
                        output_text = format!(
                            "{}\n\n[{remaining} more lines in file. Use offset={next_offset} to continue.]",
                            truncation.content
                        );
                    } else {
                        output_text = truncation.content;
                    }
                } else {
                    output_text = truncation.content;
                }

                Ok(crate::agent::types::AgentToolResult {
                    content: vec![TextOrImageContent::text(output_text)],
                    details: details.unwrap_or(Value::Null),
                    ..Default::default()
                })
            })
        }),
    }
}

#[allow(dead_code)]
fn _file_content_reference(content: FileContent) -> FileContent {
    content
}

