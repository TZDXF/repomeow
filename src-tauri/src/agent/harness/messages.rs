//! harness 自定义消息:对齐 `packages/agent/src/harness/messages.ts`。
//!
//! core 层的 `AgentMessage::Custom(serde_json::Map)` 承载 TS declaration merging
//! 注入的四个自定义 role(bashExecution/custom/branchSummary/compactionSummary)。
//! 本模块提供它们的类型化视图(serde 结构体,JSON 字段与 TS 完全兼容)、
//! `bashExecutionToText`、`create*` 构造器与 `convertToLlm`。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::llm::types::{Message, TextOrImageContent, UserContent, UserMessage};
use crate::agent::types::{AgentMessage, TypedMessage};

/// compaction 摘要注入前缀(逐字节对齐蓝本,含换行)。
pub const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";

/// compaction 摘要注入后缀。
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

/// 分支摘要注入前缀。
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";

/// 分支摘要注入后缀(注意:蓝本此处无前导换行)。
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

/// role = "bashExecution" 的自定义消息视图。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub role: String, // 恒为 "bashExecution"
    pub command: String,
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
}

/// role = "custom" 的应用自定义消息视图。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub role: String, // 恒为 "custom"
    pub custom_type: String,
    pub content: CustomMessageContent,
    pub display: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    pub timestamp: i64,
}

/// `CustomMessage.content`:`string | (TextContent | ImageContent)[]`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomMessageContent {
    Text(String),
    Blocks(Vec<TextOrImageContent>),
}

/// role = "branchSummary" 的自定义消息视图。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub role: String, // 恒为 "branchSummary"
    pub summary: String,
    pub from_id: String,
    pub timestamp: i64,
}

/// role = "compactionSummary" 的自定义消息视图。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub role: String, // 恒为 "compactionSummary"
    pub summary: String,
    pub tokens_before: i64,
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// 类型化视图提取
// ---------------------------------------------------------------------------

macro_rules! impl_from_custom {
    ($ty:ident) => {
        impl $ty {
            /// 从 `AgentMessage::Custom` 提取类型化视图;role 不匹配时返回 `None`。
            pub fn from_agent_message(message: &AgentMessage) -> Option<Self> {
                match message {
                    AgentMessage::Custom(map) => {
                        let role = map.get("role").and_then(Value::as_str)?;
                        if role == Self::ROLE {
                            serde_json::from_value(Value::Object(map.clone())).ok()
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
        }
    };
}

impl BashExecutionMessage {
    pub const ROLE: &'static str = "bashExecution";
}
impl CustomMessage {
    pub const ROLE: &'static str = "custom";
}
impl BranchSummaryMessage {
    pub const ROLE: &'static str = "branchSummary";
}
impl CompactionSummaryMessage {
    pub const ROLE: &'static str = "compactionSummary";
}

impl_from_custom!(BashExecutionMessage);
impl_from_custom!(CustomMessage);
impl_from_custom!(BranchSummaryMessage);
impl_from_custom!(CompactionSummaryMessage);

// ---------------------------------------------------------------------------
// 构造与文本化
// ---------------------------------------------------------------------------

/// 把类型化自定义消息打包回 `AgentMessage::Custom`(camelCase JSON map)。
fn custom_message_to_agent_message<T: Serialize>(message: &T) -> AgentMessage {
    let value = serde_json::to_value(message).expect("custom message must serialize");
    match value {
        Value::Object(map) => AgentMessage::Custom(map),
        _ => unreachable!("custom messages serialize to objects"),
    }
}

/// 对齐 TS `bashExecutionToText`。
pub fn bash_execution_to_text(msg: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", msg.command);
    if !msg.output.is_empty() {
        text.push_str(&format!("```\n{}\n```", msg.output));
    } else {
        text.push_str("(no output)");
    }
    if msg.cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(exit_code) = msg.exit_code {
        if exit_code != 0 {
            text.push_str(&format!("\n\nCommand exited with code {exit_code}"));
        }
    }
    if msg.truncated {
        if let Some(full_output_path) = &msg.full_output_path {
            text.push_str(&format!(
                "\n\n[Output truncated. Full output: {full_output_path}]"
            ));
        }
    }
    text
}

/// 构造 branchSummary 自定义消息(时间戳为 Unix 毫秒;蓝本另接受日期串,见报告偏差)。
pub fn create_branch_summary_message(summary: impl Into<String>, from_id: impl Into<String>, timestamp: i64) -> AgentMessage {
    custom_message_to_agent_message(&BranchSummaryMessage {
        role: BranchSummaryMessage::ROLE.to_string(),
        summary: summary.into(),
        from_id: from_id.into(),
        timestamp,
    })
}

/// 构造 compactionSummary 自定义消息。
pub fn create_compaction_summary_message(summary: impl Into<String>, tokens_before: i64, timestamp: i64) -> AgentMessage {
    custom_message_to_agent_message(&CompactionSummaryMessage {
        role: CompactionSummaryMessage::ROLE.to_string(),
        summary: summary.into(),
        tokens_before,
        timestamp,
    })
}

/// 构造 custom 自定义消息。
pub fn create_custom_message(
    custom_type: impl Into<String>,
    content: CustomMessageContent,
    display: bool,
    details: Option<Value>,
    timestamp: i64,
) -> AgentMessage {
    custom_message_to_agent_message(&CustomMessage {
        role: CustomMessage::ROLE.to_string(),
        custom_type: custom_type.into(),
        content,
        display,
        details,
        timestamp,
    })
}

/// 构造 bashExecution 自定义消息。
pub fn create_bash_execution_message(message: BashExecutionMessage) -> AgentMessage {
    debug_assert_eq!(message.role, BashExecutionMessage::ROLE);
    custom_message_to_agent_message(&message)
}

// ---------------------------------------------------------------------------
// convertToLlm
// ---------------------------------------------------------------------------

/// `AgentMessage[]` → LLM `Message[]`(对齐 TS `convertToLlm`)。
///
/// 已知 core role 直接透传;自定义 role 按蓝本映射为 user 文本/块;未知 role 过滤。
pub fn convert_to_llm(messages: Vec<AgentMessage>) -> Vec<Message> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::Message(typed) => match typed {
                TypedMessage::User(user) => Some(Message::User(user)),
                TypedMessage::Assistant(assistant) => Some(Message::Assistant(assistant)),
                TypedMessage::ToolResult(result) => Some(Message::ToolResult(result)),
            },
            AgentMessage::Custom(map) => {
                let role = map.get("role").and_then(Value::as_str).unwrap_or_default();
                match role {
                    "bashExecution" => {
                        let msg: BashExecutionMessage =
                            serde_json::from_value(Value::Object(map)).ok()?;
                        if msg.exclude_from_context.unwrap_or(false) {
                            return None;
                        }
                        Some(Message::User(UserMessage {
                            role: "user".to_string(),
                            content: UserContent::Blocks(vec![TextOrImageContent::text(
                                bash_execution_to_text(&msg),
                            )]),
                            timestamp: msg.timestamp,
                        }))
                    }
                    "custom" => {
                        let msg: CustomMessage = serde_json::from_value(Value::Object(map)).ok()?;
                        let content = match msg.content {
                            CustomMessageContent::Text(text) => {
                                vec![TextOrImageContent::text(text)]
                            }
                            CustomMessageContent::Blocks(blocks) => blocks,
                        };
                        Some(Message::User(UserMessage {
                            role: "user".to_string(),
                            content: UserContent::Blocks(content),
                            timestamp: msg.timestamp,
                        }))
                    }
                    "branchSummary" => {
                        let msg: BranchSummaryMessage =
                            serde_json::from_value(Value::Object(map)).ok()?;
                        Some(Message::User(UserMessage {
                            role: "user".to_string(),
                            content: UserContent::Blocks(vec![TextOrImageContent::text(format!(
                                "{}{}{}",
                                BRANCH_SUMMARY_PREFIX, msg.summary, BRANCH_SUMMARY_SUFFIX
                            ))]),
                            timestamp: msg.timestamp,
                        }))
                    }
                    "compactionSummary" => {
                        let msg: CompactionSummaryMessage =
                            serde_json::from_value(Value::Object(map)).ok()?;
                        Some(Message::User(UserMessage {
                            role: "user".to_string(),
                            content: UserContent::Blocks(vec![TextOrImageContent::text(format!(
                                "{}{}{}",
                                COMPACTION_SUMMARY_PREFIX, msg.summary, COMPACTION_SUMMARY_SUFFIX
                            ))]),
                            timestamp: msg.timestamp,
                        }))
                    }
                    _ => None,
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_roles_pass_through() {
        let messages = vec![
            AgentMessage::user_text("hi", 1),
            AgentMessage::Custom(
                json!({"role": "mystery", "x": 1})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        ];
        let converted = convert_to_llm(messages);
        assert_eq!(converted.len(), 1);
        match &converted[0] {
            Message::User(user) => assert_eq!(user.content.to_plain_text(), "hi"),
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn bash_execution_maps_to_user_text() {
        let message = create_bash_execution_message(BashExecutionMessage {
            role: "bashExecution".into(),
            command: "ls -la".into(),
            output: "total 1".into(),
            exit_code: Some(1),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 42,
            exclude_from_context: None,
        });
        let converted = convert_to_llm(vec![message]);
        assert_eq!(converted.len(), 1);
        let Message::User(user) = &converted[0] else {
            panic!("expected user message");
        };
        let expected = "Ran `ls -la`\n```\ntotal 1\n```\n\nCommand exited with code 1";
        assert_eq!(user.content.to_plain_text(), expected);
        assert_eq!(user.timestamp, 42);
    }

    #[test]
    fn bash_execution_excluded_from_context() {
        let message = create_bash_execution_message(BashExecutionMessage {
            role: "bashExecution".into(),
            command: "ls".into(),
            output: String::new(),
            exit_code: None,
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 1,
            exclude_from_context: Some(true),
        });
        assert!(convert_to_llm(vec![message]).is_empty());
    }

    #[test]
    fn summary_messages_wrap_with_prefix_and_suffix() {
        let messages = vec![
            create_branch_summary_message("branch body", "entry-1", 7),
            create_compaction_summary_message("compaction body", 1200, 8),
        ];
        let converted = convert_to_llm(messages);
        assert_eq!(converted.len(), 2);
        let Message::User(user) = &converted[0] else {
            panic!("expected user message");
        };
        assert_eq!(
            user.content.to_plain_text(),
            format!("{BRANCH_SUMMARY_PREFIX}branch body{BRANCH_SUMMARY_SUFFIX}")
        );
        let Message::User(user) = &converted[1] else {
            panic!("expected user message");
        };
        assert_eq!(
            user.content.to_plain_text(),
            format!("{COMPACTION_SUMMARY_PREFIX}compaction body{COMPACTION_SUMMARY_SUFFIX}")
        );
    }

    #[test]
    fn custom_message_content_supports_string_and_blocks() {
        let as_string = create_custom_message("note", CustomMessageContent::Text("plain".into()), true, None, 3);
        let as_blocks = create_custom_message(
            "note",
            CustomMessageContent::Blocks(vec![TextOrImageContent::text("rich")]),
            true,
            None,
            3,
        );
        let converted = convert_to_llm(vec![as_string, as_blocks]);
        assert_eq!(converted.len(), 2);
        let Message::User(user) = &converted[0] else {
            panic!("expected user message");
        };
        assert_eq!(user.content.to_plain_text(), "plain");
        let Message::User(user) = &converted[1] else {
            panic!("expected user message");
        };
        assert_eq!(user.content.to_plain_text(), "rich");
    }

    #[test]
    fn custom_message_json_shape_matches_ts() {
        let message = create_custom_message(
            "note",
            CustomMessageContent::Text("body".into()),
            false,
            Some(json!({"k": 1})),
            99,
        );
        let AgentMessage::Custom(map) = &message else {
            panic!("expected custom map");
        };
        assert_eq!(map.get("role").and_then(Value::as_str), Some("custom"));
        assert_eq!(map.get("customType").and_then(Value::as_str), Some("note"));
        assert_eq!(map.get("display").and_then(Value::as_bool), Some(false));
        assert_eq!(map.get("timestamp").and_then(Value::as_i64), Some(99));

        let view = CustomMessage::from_agent_message(&message).unwrap();
        assert_eq!(view.details, Some(json!({"k": 1})));
    }

    #[test]
    fn bash_execution_to_text_variants() {
        let no_output = BashExecutionMessage {
            role: "bashExecution".into(),
            command: "echo".into(),
            output: String::new(),
            exit_code: None,
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 0,
            exclude_from_context: None,
        };
        assert_eq!(bash_execution_to_text(&no_output), "Ran `echo`\n(no output)");

        let truncated = BashExecutionMessage {
            role: "bashExecution".into(),
            command: "cat".into(),
            output: "x".into(),
            exit_code: None,
            cancelled: true,
            truncated: true,
            full_output_path: Some("/tmp/full.log".into()),
            timestamp: 0,
            exclude_from_context: None,
        };
        assert_eq!(
            bash_execution_to_text(&truncated),
            "Ran `cat`\n```\nx\n```\n\n(command cancelled)\n\n[Output truncated. Full output: /tmp/full.log]"
        );
    }
}
