//! ls 工具:对齐 `packages/coding-agent/src/core/tools/ls.ts`。
//!
//! 单层目录列举(不递归):包含 dotfiles、大小写不敏感排序、目录加 `/` 后缀、
//! 默认 500 条、总量 50KB 截断。经 [`ExecutionEnv::FileSystem::list_dir`] 取项,
//! 不绕过抽象。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::harness::tools::path_utils::resolve_tool_path;
use crate::agent::harness::types::{ExecutionEnv, FileKind, SimpleError};
use crate::agent::harness::utils::truncate::{
    format_size, truncate_head, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES,
};
use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionError};

const DEFAULT_LIMIT: usize = 500;

/// ls 工具参数(对齐 TS `LsToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LsToolInput {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<f64>,
}

/// ls 工具详情(对齐 TS `LsToolDetails`)。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LsToolDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<TruncationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_limit_reached: Option<usize>,
}

/// 创建 ls 工具(构造时捕获 env;返回 core AgentTool)。
pub fn create_ls_tool(env: Arc<dyn ExecutionEnv>) -> AgentTool {
    AgentTool {
        name: "ls".to_string(),
        label: "ls".to_string(),
        description: format!(
            "List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to {DEFAULT_LIMIT} entries or {}KB (whichever is hit first).",
            DEFAULT_MAX_BYTES / 1024
        ),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (default: current directory)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of entries to return (default: 500)"
                }
            }
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, params, signal, _on_update| {
            let env = env.clone();
            Box::pin(async move {
                let input: LsToolInput = serde_json::from_value(params)
                    .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))?;

                let path_input = input.path.as_deref().unwrap_or("").trim();
                let dir_path = resolve_tool_path(
                    env.as_ref(),
                    if path_input.is_empty() { "." } else { path_input },
                    signal.clone(),
                )
                .await
                .map_err(|error| {
                    ToolExecutionError::from(SimpleError::new(error.to_string()))
                })?;
                if !env.exists(dir_path.clone(), signal.clone()).await.unwrap_or(false) {
                    return Err(ToolExecutionError::from(SimpleError::new(format!(
                        "Path not found: {dir_path}"
                    ))));
                }
                let info = env.file_info(dir_path.clone()).await.map_err(|error| {
                    ToolExecutionError::from(SimpleError::new(error.to_string()))
                })?;
                if info.kind != FileKind::Directory {
                    return Err(ToolExecutionError::from(SimpleError::new(format!(
                        "Not a directory: {dir_path}"
                    ))));
                }
                let mut entries = env
                    .list_dir(dir_path.clone(), signal.clone())
                    .await
                    .map_err(|error| {
                        ToolExecutionError::from(SimpleError::new(format!(
                            "Cannot read directory: {error}"
                        )))
                    })?;
                entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

                let effective_limit = input
                    .limit
                    .map(|limit| limit.max(0.0) as usize)
                    .unwrap_or(DEFAULT_LIMIT);
                let mut results: Vec<String> = Vec::new();
                let mut entry_limit_reached = false;
                for entry in entries {
                    if results.len() >= effective_limit {
                        entry_limit_reached = true;
                        break;
                    }
                    let suffix = if entry.kind == FileKind::Directory { "/" } else { "" };
                    results.push(format!("{}{suffix}", entry.name));
                }

                if results.is_empty() {
                    return Ok(AgentToolResult {
                        content: vec![crate::agent::types::TextOrImageContent::text(
                            "(empty directory)",
                        )],
                        ..Default::default()
                    });
                }

                let raw_output = results.join("\n");
                let truncation = truncate_head(
                    &raw_output,
                    TruncationOptions {
                        max_lines: Some(usize::MAX),
                        max_bytes: None,
                    },
                );
                let mut output = truncation.content.clone();
                let mut notices: Vec<String> = Vec::new();
                if entry_limit_reached {
                    notices.push(format!(
                        "{effective_limit} entries limit reached. Use limit={} for more",
                        effective_limit * 2
                    ));
                }
                if truncation.truncated {
                    notices.push(format!("{} limit reached", format_size(DEFAULT_MAX_BYTES)));
                }
                if !notices.is_empty() {
                    output.push_str(&format!("\n\n[{}]", notices.join(". ")));
                }

                let has_details = entry_limit_reached || truncation.truncated;
                let details = LsToolDetails {
                    truncation: if truncation.truncated {
                        Some(truncation.clone())
                    } else {
                        None
                    },
                    entry_limit_reached: if entry_limit_reached {
                        Some(effective_limit)
                    } else {
                        None
                    },
                };
                Ok(AgentToolResult {
                    content: vec![crate::agent::types::TextOrImageContent::text(output)],
                    details: if has_details {
                        serde_json::to_value(&details).unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    },
                    ..Default::default()
                })
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::harness::env::TokioEnv;
    use crate::agent::types::TextOrImageContent;

    fn text_of(result: &AgentToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|content| match content {
                TextOrImageContent::Text { text, .. } => Some(text.clone()),
                TextOrImageContent::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    struct TempDir {
        root: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "repomeow-ls-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn env(&self) -> Arc<dyn ExecutionEnv> {
            Arc::new(TokioEnv::new(self.root.clone()))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[tokio::test]
    async fn lists_sorted_with_dir_suffix_and_dotfiles() {
        let dir = TempDir::new();
        std::fs::write(dir.root.join("zeta.txt"), "").unwrap();
        std::fs::write(dir.root.join(".dotfile"), "").unwrap();
        std::fs::create_dir_all(dir.root.join("Alpha")).unwrap();
        std::fs::write(dir.root.join("Alpha").join("inner.txt"), "").unwrap();
        let tool = create_ls_tool(dir.env());
        let result = (tool.execute)("call-1".to_string(), json!({}), None, None)
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(text.contains(".dotfile"), "{text}");
        assert!(text.contains("Alpha/"), "{text}");
        assert!(text.contains("zeta.txt"), "{text}");
        let lines: Vec<&str> = text.lines().collect();
        let alpha_pos = lines.iter().position(|l| l.starts_with("Alpha")).unwrap();
        let dot_pos = lines
            .iter()
            .position(|l| l.starts_with(".dotfile"))
            .unwrap();
        let zeta_pos = lines
            .iter()
            .position(|l| l.starts_with("zeta.txt"))
            .unwrap();
        // 大小写不敏感排序:.dotfile < Alpha < zeta。
        assert!(dot_pos < alpha_pos && alpha_pos < zeta_pos, "{text}");
    }

    #[tokio::test]
    async fn empty_directory_message() {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.root.join("empty")).unwrap();
        let tool = create_ls_tool(dir.env());
        let result = (tool.execute)("call-1".to_string(), json!({ "path": "empty" }), None, None)
            .await
            .unwrap();
        assert_eq!(text_of(&result), "(empty directory)");
    }

    #[tokio::test]
    async fn limit_reached_notice() {
        let dir = TempDir::new();
        for name in ["a", "b", "c"] {
            std::fs::write(dir.root.join(format!("{name}.txt")), "").unwrap();
        }
        let tool = create_ls_tool(dir.env());
        let result = (tool.execute)("call-1".to_string(), json!({ "limit": 2 }), None, None)
            .await
            .unwrap();
        assert_eq!(result.details["entryLimitReached"], 2);
        assert!(text_of(&result).contains("2 entries limit reached"));
    }

    #[tokio::test]
    async fn file_and_missing_path_errors() {
        let dir = TempDir::new();
        std::fs::write(dir.root.join("file.txt"), "x").unwrap();
        let tool = create_ls_tool(dir.env());
        let error = (tool.execute)(
            "call-1".to_string(),
            json!({ "path": "file.txt" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Not a directory"), "{error}");
        let error = (tool.execute)(
            "call-2".to_string(),
            json!({ "path": "missing" }),
            None,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Path not found"), "{error}");
    }
}
