//! compaction 辅助:对齐 `packages/agent/src/harness/compaction/utils.ts`。

use serde_json::Value;

use crate::agent::llm::types::{AssistantContent, Message, UserContent};
use crate::agent::types::{AgentMessage, TypedMessage};

/// 会话分支或压缩范围内触碰的文件(对齐 TS `FileOperations`)。
#[derive(Clone, Debug, Default)]
pub struct FileOperations {
    /// 读过但未必修改。
    pub read: std::collections::BTreeSet<String>,
    /// 由整文件写操作写入。
    pub written: std::collections::BTreeSet<String>,
    /// 由编辑操作修改。
    pub edited: std::collections::BTreeSet<String>,
}

/// 创建空累加器(对齐 TS `createFileOps`)。
pub fn create_file_ops() -> FileOperations {
    FileOperations::default()
}

/// 从助手消息的工具调用累加文件操作(对齐 TS `extractFileOpsFromMessage`)。
pub fn extract_file_ops_from_message(message: &AgentMessage, file_ops: &mut FileOperations) {
    let AgentMessage::Message(TypedMessage::Assistant(assistant)) = message else {
        return;
    };
    for block in &assistant.content {
        let AssistantContent::ToolCall(tool_call) = block else {
            continue;
        };
        let Some(path) = tool_call.arguments.get("path").and_then(Value::as_str) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        match tool_call.name.as_str() {
            "read" => {
                file_ops.read.insert(path.to_string());
            }
            "write" => {
                file_ops.written.insert(path.to_string());
            }
            "edit" => {
                file_ops.edited.insert(path.to_string());
            }
            _ => {}
        }
    }
}

/// 计算排序后的只读/已修改文件清单(对齐 TS `computeFileLists`)。
pub fn compute_file_lists(file_ops: &FileOperations) -> (Vec<String>, Vec<String>) {
    let mut modified: Vec<String> = file_ops
        .edited
        .iter()
        .chain(file_ops.written.iter())
        .cloned()
        .collect();
    modified.sort();
    modified.dedup();
    let read_only: Vec<String> = file_ops
        .read
        .iter()
        .filter(|file| !modified.contains(file))
        .cloned()
        .collect();
    (read_only, modified)
}

/// 把文件清单格式化为摘要元数据标签(对齐 TS `formatFileOperations`)。
pub fn format_file_operations(read_files: &[String], modified_files: &[String]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !read_files.is_empty() {
        sections.push(format!(
            "<read-files>\n{}\n</read-files>",
            read_files.join("\n")
        ));
    }
    if !modified_files.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified_files.join("\n")
        ));
    }
    if sections.is_empty() {
        return String::new();
    }
    format!("\n\n{}", sections.join("\n\n"))
}

const TOOL_RESULT_MAX_CHARS: usize = 2000;

fn safe_json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_string())
}

fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated_chars = text.chars().count() - max_chars;
    let head: String = text.chars().take(max_chars).collect();
    format!("{head}\n\n[... {truncated_chars} more characters truncated]")
}

/// 提取消息中的文本内容(对齐 pi-ai `contentText`;以 "\n" 连接)。
pub fn content_text_of_user(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                crate::agent::llm::types::TextOrImageContent::Text { text, .. } => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// 把 LLM 消息序列化为纯文本供摘要 prompt(对齐 TS `serializeConversation`)。
pub fn serialize_conversation(messages: &[Message]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for message in messages {
        match message {
            Message::User(user) => {
                let content = content_text_of_user(&user.content);
                if !content.is_empty() {
                    parts.push(format!("[User]: {content}"));
                }
            }
            Message::Assistant(assistant) => {
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut tool_calls: Vec<String> = Vec::new();
                for block in &assistant.content {
                    match block {
                        AssistantContent::Thinking { thinking, .. } => {
                            thinking_parts.push(thinking.clone());
                        }
                        AssistantContent::ToolCall(tool_call) => {
                            let args: Vec<String> = tool_call
                                .arguments
                                .iter()
                                .map(|(key, value)| format!("{key}={}", safe_json_stringify(value)))
                                .collect();
                            tool_calls.push(format!("{}({})", tool_call.name, args.join(", ")));
                        }
                        AssistantContent::Text { .. } => {}
                    }
                }
                if !thinking_parts.is_empty() {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        thinking_parts.join("\n")
                    ));
                }
                let has_text = assistant
                    .content
                    .iter()
                    .any(|block| matches!(block, AssistantContent::Text { .. }));
                if has_text {
                    let text = assistant
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AssistantContent::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    parts.push(format!("[Assistant]: {text}"));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Message::ToolResult(result) => {
                let content: String = result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        crate::agent::llm::types::TextOrImageContent::Text { text, .. } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !content.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&content, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
        }
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::{test_assistant, test_tool_call};
    use crate::agent::llm::types::StopReason;
    use serde_json::json;

    #[test]
    fn extracts_file_ops_from_assistant_tool_calls() {
        let assistant = test_assistant(
            vec![
                AssistantContent::ToolCall(test_tool_call("1", "read", json!({"path": "/a.txt"}))),
                AssistantContent::ToolCall(test_tool_call("2", "write", json!({"path": "/b.txt"}))),
                AssistantContent::ToolCall(test_tool_call("3", "edit", json!({"path": "/b.txt"}))),
                AssistantContent::ToolCall(test_tool_call("4", "bash", json!({"command": "ls"}))),
            ],
            StopReason::ToolUse,
        );
        let mut file_ops = create_file_ops();
        extract_file_ops_from_message(
            &AgentMessage::Message(TypedMessage::Assistant(assistant)),
            &mut file_ops,
        );
        let (read, modified) = compute_file_lists(&file_ops);
        assert_eq!(read, vec!["/a.txt".to_string()]);
        assert_eq!(modified, vec!["/b.txt".to_string()]);
    }

    #[test]
    fn formats_file_operation_sections() {
        assert_eq!(format_file_operations(&[], &[]), "");
        let output = format_file_operations(
            &["/a.txt".to_string()],
            &["/b.txt".to_string(), "/c.txt".to_string()],
        );
        assert_eq!(
            output,
            "\n\n<read-files>\n/a.txt\n</read-files>\n\n<modified-files>\n/b.txt\n/c.txt\n</modified-files>"
        );
    }

    #[test]
    fn serializes_conversation_sections() {
        let user = crate::agent::llm::types::UserMessage {
            role: "user".to_string(),
            content: UserContent::Text("hello there".to_string()),
            timestamp: 1,
        };
        let assistant = test_assistant(
            vec![
                AssistantContent::Thinking {
                    thinking: "hmm".to_string(),
                    thinking_signature: None,
                    redacted: false,
                },
                AssistantContent::text("answer"),
                AssistantContent::ToolCall(test_tool_call(
                    "t1",
                    "read",
                    json!({"path": "/x", "n": 3}),
                )),
            ],
            StopReason::Stop,
        );
        let output = serialize_conversation(&[Message::User(user), Message::Assistant(assistant)]);
        assert!(output.contains("[User]: hello there"));
        assert!(output.contains("[Assistant thinking]: hmm"));
        assert!(output.contains("[Assistant]: answer"));
        assert!(output.contains(r#"[Assistant tool calls]: read(path="/x", n=3)"#));
    }

    #[test]
    fn tool_results_are_truncated_in_summary() {
        let long_text = "x".repeat(3000);
        let result = crate::agent::llm::types::ToolResultMessage {
            role: "toolResult".to_string(),
            tool_call_id: "t".to_string(),
            tool_name: "read".to_string(),
            content: vec![crate::agent::llm::types::TextOrImageContent::text(
                long_text,
            )],
            details: None,
            usage: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        };
        let output = serialize_conversation(&[Message::ToolResult(result)]);
        assert!(output.contains("more characters truncated"));
    }
}
