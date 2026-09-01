//! write 工具:对齐 `packages/agent/src/harness/tools/write.ts`。

use std::sync::Arc;

use serde_json::json;

use crate::agent::harness::tools::file_mutation_queue::with_file_mutation_queue;
use crate::agent::harness::tools::path_utils::resolve_tool_path;
use crate::agent::harness::types::{ExecutionEnv, FileContent, SimpleError};
use crate::agent::types::{AbortSignal, AgentTool, AgentToolResult, ToolExecutionError};

/// write 工具参数(对齐 TS `WriteToolInput`)。
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteToolInput {
    pub path: String,
    pub content: String,
}

/// 创建 write 工具(自动建父目录;返回 core AgentTool)。
pub fn create_write_tool(env: Arc<dyn ExecutionEnv>) -> AgentTool {
    AgentTool {
        name: "write".to_string(),
        label: "write".to_string(),
        description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        }),
        execution_mode: None,
        prepare_arguments: None,
        execute: Arc::new(move |_tool_call_id, params, signal, _on_update| {
            let env = env.clone();
            Box::pin(async move {
                let input: WriteToolInput = serde_json::from_value(params)
                    .map_err(|error| ToolExecutionError::from(SimpleError::new(error.to_string())))?;
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
                    let signal: Option<AbortSignal> = signal.clone();
                    move || {
                        let env = env.clone();
                        let input = input.clone();
                        let signal = signal.clone();
                        let absolute_path = closure_path.clone();
                        Box::pin(async move {
                            if signal.as_ref().map(|s| s.is_cancelled()).unwrap_or(false) {
                                return Err(ToolExecutionError::from(SimpleError::new("Operation aborted")));
                            }
                            env.write_file(
                                absolute_path.clone(),
                                FileContent::Text(input.content.clone()),
                                signal.clone(),
                            )
                            .await
                            .map_err(|error| {
                                ToolExecutionError::from(SimpleError::new(error.to_string()))
                            })?;
                            if signal.as_ref().map(|s| s.is_cancelled()).unwrap_or(false) {
                                return Err(ToolExecutionError::from(SimpleError::new("Operation aborted")));
                            }
                            Ok(AgentToolResult::text(format!(
                                "Successfully wrote {} bytes to {}",
                                input.content.len(),
                                input.path
                            )))
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
