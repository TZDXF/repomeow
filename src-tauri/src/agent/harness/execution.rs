//! 工具执行收尾:对齐 `packages/agent/src/harness/execution/tools.ts` 的
//! `finalizeToolCall` / `createToolResultMessage` 核心形状。
//!
//! P4 保留 execution 为单文件;tool registry、effect gate 与完整参数校验仍在
//! 本仓库既有 runtime/tools 路径中运行,待后续逐步接线。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 参数校验通过、可发布 durable intent 并执行的工具调用。
#[derive(Clone, Debug)]
pub struct ClearedToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
}

/// 阶段二原始工具输出。
#[derive(Clone, Debug)]
pub struct ExecutedToolCall {
    pub result: ToolOutput,
    pub is_error: bool,
}

/// after-tool hook 的字段级补丁。
#[derive(Clone, Debug, Default)]
pub struct AfterToolPatch {
    pub content: Option<Vec<Value>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
}

/// 工具输出的 provider 内容块与自定义 details。
#[derive(Clone, Debug)]
pub struct ToolOutput {
    /// text/image 等内容块;此处保留 JSON 形状以兼容既有工具适配层。
    pub content: Vec<Value>,
    pub details: Option<Value>,
}

/// 完成收尾、可转为 durable tool-result message 的工具调用。
#[derive(Clone, Debug)]
pub struct FinalizedToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub result: Option<ToolOutput>,
    pub is_error: bool,
}

/// 应用 after-tool patch 并生成 finalized call。
pub fn finalize_tool_call(
    call: ClearedToolCall,
    executed: ExecutedToolCall,
    patch: Option<AfterToolPatch>,
) -> FinalizedToolCall {
    let (result, is_error) = match patch {
        Some(patch) => {
            let result = ToolOutput {
                content: patch.content.unwrap_or(executed.result.content),
                details: patch.details.or(executed.result.details),
            };
            let is_error = patch.is_error.unwrap_or(executed.is_error);
            (result, is_error)
        }
        None => (executed.result, executed.is_error),
    };
    FinalizedToolCall {
        tool_call_id: call.tool_call_id,
        tool_name: call.tool_name,
        args: call.args,
        result: Some(result),
        is_error,
    }
}

/// 将 finalized 输出转换为 provider-facing transcript message 形状。
pub fn create_tool_result_message(call: &FinalizedToolCall) -> Value {
    serde_json::json!({
        "role": "toolResult",
        "toolCallId": call.tool_call_id,
        "toolName": call.tool_name,
        "content": call.result.as_ref().map(|result| result.content.clone()).unwrap_or_default(),
        "isError": call.is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finalize_applies_after_tool_patch() {
        let call = ClearedToolCall {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            args: json!({}),
        };
        let executed = ExecutedToolCall {
            result: ToolOutput {
                content: vec![json!({"type": "text", "text": "old"})],
                details: Some(json!({"old": true})),
            },
            is_error: false,
        };
        let patch = AfterToolPatch {
            content: Some(vec![json!({"type": "text", "text": "patched"})]),
            details: None,
            is_error: Some(true),
        };

        let finalized = finalize_tool_call(call, executed, Some(patch));
        assert_eq!(
            finalized.result.as_ref().unwrap().content,
            vec![json!({"type": "text", "text": "patched"})]
        );
        assert_eq!(
            finalized.result.as_ref().unwrap().details,
            Some(json!({"old": true}))
        );
        assert!(finalized.is_error);
    }

    #[test]
    fn creates_canonical_tool_result_json() {
        let call = FinalizedToolCall {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            args: json!({}),
            result: Some(ToolOutput {
                content: vec![json!({"type": "text", "text": "ok"})],
                details: None,
            }),
            is_error: false,
        };

        let message = create_tool_result_message(&call);
        assert_eq!(
            message,
            json!({
                "role": "toolResult",
                "toolCallId": "call-1",
                "toolName": "read",
                "content": [{"type": "text", "text": "ok"}],
                "isError": false,
            })
        );
    }
}
