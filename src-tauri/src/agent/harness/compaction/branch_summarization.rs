//! 分支摘要:对齐 `packages/agent/src/harness/compaction/branch-summarization.ts`。

use serde::{Deserialize, Serialize};

use crate::agent::llm::types::{
    Model, StopReason, TextOrImageContent, Usage, UserContent, UserMessage,
};
use crate::agent::types::{AgentMessage, StreamFn};

use crate::agent::agent_loop::now_ms;
use crate::agent::harness::compaction::compaction::{
    complete_simple_with_retries, estimate_tokens, SUMMARIZATION_SYSTEM_PROMPT,
};
use crate::agent::harness::compaction::utils::{
    compute_file_lists, create_file_ops, extract_file_ops_from_message, format_file_operations,
    serialize_conversation, FileOperations,
};
use crate::agent::harness::messages::convert_to_llm;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{
    BranchBounds, BranchQuery, Entry, SessionError, SessionErrorCode, SessionTree,
};
use crate::agent::harness::types::{err, ok, BranchSummaryError, BranchSummaryErrorCode, Result};

/// 生成的分支摘要数据(对齐 TS `BranchSummaryResult`)。
#[derive(Clone, Debug)]
pub struct BranchSummaryResult {
    pub summary: String,
    pub usage: Option<Usage>,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// 分支摘要条目的文件操作明细(对齐 TS `BranchSummaryDetails`)。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// 准备好的分支内容(对齐 TS `BranchPreparation`)。
#[derive(Clone, Debug)]
pub struct BranchPreparation {
    pub messages: Vec<AgentMessage>,
    pub file_ops: FileOperations,
    pub total_tokens: i64,
}

/// 收集的条目(对齐 TS `CollectEntriesResult`)。
#[derive(Clone, Debug)]
pub struct CollectEntriesResult {
    /// 按时间顺序待摘要的条目。
    pub entries: Vec<Entry>,
    /// 旧叶与目标条目的最深公共祖先。
    pub common_ancestor_id: Option<String>,
}

/// 分支摘要生成选项(对齐 TS `GenerateBranchSummaryOptions`;`models` 简化为
/// 直接传 [`StreamFn`])。
pub struct GenerateBranchSummaryOptions<'a> {
    pub stream_fn: &'a StreamFn,
    pub model: &'a Model,
    pub custom_instructions: Option<&'a str>,
    /// 用自定义指令替换默认 prompt 而非追加。
    pub replace_instructions: bool,
    /// prompt 与模型输出保留的 token;默认 16384。
    pub reserve_tokens: i64,
}

const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";

const BRANCH_SUMMARY_PROMPT: &str = "Create a structured summary of this conversation branch for context when returning later.\n\nUse this EXACT format:\n\n## Goal\n[What was the user trying to accomplish in this branch?]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Work that was started but not finished]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [What should happen next to continue this work]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// 收集导航离开前需要摘要的条目(对齐 TS `collectEntriesForBranchSummary`)。
pub async fn collect_entries_for_branch_summary(
    session: &Session,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> Result<CollectEntriesResult, SessionError> {
    let Some(old_leaf_id) = old_leaf_id else {
        return ok(CollectEntriesResult {
            entries: Vec::new(),
            common_ancestor_id: None,
        });
    };
    let old_path: std::collections::HashSet<String> = session
        .find_entries_on_branch(BranchQuery {
            query: Default::default(),
            bounds: BranchBounds {
                start: Some(old_leaf_id.to_string()),
                ..Default::default()
            },
        })
        .await?
        .into_iter()
        .map(|entry| entry.id().to_string())
        .collect();
    let target_path = session
        .find_entries_on_branch(BranchQuery {
            query: Default::default(),
            bounds: BranchBounds {
                start: Some(target_id.to_string()),
                ..Default::default()
            },
        })
        .await?;
    let mut common_ancestor_id: Option<String> = None;
    for entry in &target_path {
        if old_path.contains(entry.id()) {
            common_ancestor_id = Some(entry.id().to_string());
            break;
        }
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut current: Option<String> = Some(old_leaf_id.to_string());

    while let Some(current_id) = current {
        if Some(&current_id) == common_ancestor_id.as_ref() {
            break;
        }
        let entry = session.get_entry(current_id.clone()).await.ok_or_else(|| {
            SessionError::new(
                SessionErrorCode::InvalidEntry,
                format!("Entry {current_id} not found"),
            )
        })?;
        current = entry.parent_id().map(str::to_string);
        entries.push(entry);
    }
    entries.reverse();

    ok(CollectEntriesResult {
        entries,
        common_ancestor_id,
    })
}

fn get_message_from_entry(entry: &Entry) -> Option<AgentMessage> {
    match entry {
        Entry::Message(message_entry) => {
            if message_entry.message.role_name() == "toolResult" {
                return None;
            }
            Some(message_entry.message.clone())
        }
        Entry::BranchSummary(branch) => Some(
            crate::agent::harness::messages::create_branch_summary_message(
                branch.summary.clone(),
                branch.from_id.clone(),
                branch.timestamp,
            ),
        ),
        Entry::Compaction(compaction) => Some(
            crate::agent::harness::messages::create_compaction_summary_message(
                compaction.summary.clone(),
                compaction.tokens_before,
                compaction.timestamp,
            ),
        ),
        _ => None,
    }
}

/// 在可选 token 预算内准备待摘要的分支条目
/// (对齐 TS `prepareBranchEntries`;从最新向回收集,compaction/branch_summary
/// 条目允许超出预算 10% 仍纳入)。
pub fn prepare_branch_entries(entries: &[Entry], token_budget: i64) -> BranchPreparation {
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut file_ops = create_file_ops();
    let mut total_tokens = 0i64;
    for entry in entries {
        if let Entry::BranchSummary(branch) = entry {
            if let Some(details) = &branch.details {
                if let Ok(parsed) = serde_json::from_value::<BranchSummaryDetails>(details.clone())
                {
                    for file in parsed.read_files {
                        file_ops.read.insert(file);
                    }
                    for file in parsed.modified_files {
                        file_ops.edited.insert(file);
                    }
                }
            }
        }
    }
    for entry in entries.iter().rev() {
        let Some(message) = get_message_from_entry(entry) else {
            continue;
        };
        extract_file_ops_from_message(&message, &mut file_ops);

        let tokens = estimate_tokens(&message);
        if token_budget > 0 && total_tokens + tokens > token_budget {
            if matches!(entry, Entry::Compaction(_) | Entry::BranchSummary(_)) {
                if total_tokens < (token_budget as f64 * 0.9) as i64 {
                    messages.insert(0, message);
                    total_tokens += tokens;
                }
            }
            break;
        }

        messages.insert(0, message);
        total_tokens += tokens;
    }

    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

fn content_text_of_assistant(message: &crate::agent::llm::types::AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::agent::llm::types::AssistantContent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 为被放弃的分支条目生成摘要(对齐 TS `generateBranchSummary`)。
pub async fn generate_branch_summary(
    entries: &[Entry],
    options: GenerateBranchSummaryOptions<'_>,
) -> Result<BranchSummaryResult, BranchSummaryError> {
    let GenerateBranchSummaryOptions {
        stream_fn,
        model,
        custom_instructions,
        replace_instructions,
        reserve_tokens,
    } = options;
    let context_window = if model.context_window > 0 {
        model.context_window
    } else {
        128_000
    };
    let token_budget = context_window - reserve_tokens;

    let preparation = prepare_branch_entries(entries, token_budget);

    if preparation.messages.is_empty() {
        return ok(BranchSummaryResult {
            summary: "No content to summarize".to_string(),
            usage: None,
            read_files: Vec::new(),
            modified_files: Vec::new(),
        });
    }
    let llm_messages = convert_to_llm(preparation.messages);
    let conversation_text = serialize_conversation(&llm_messages);
    let instructions = if replace_instructions && custom_instructions.is_some() {
        custom_instructions.unwrap_or_default().to_string()
    } else if let Some(custom_instructions) = custom_instructions {
        format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {custom_instructions}")
    } else {
        BRANCH_SUMMARY_PROMPT.to_string()
    };
    let prompt_text =
        format!("<conversation>\n{conversation_text}\n</conversation>\n\n{instructions}");

    let summarization_messages = vec![crate::agent::llm::types::Message::User(UserMessage {
        role: "user".to_string(),
        content: UserContent::Blocks(vec![TextOrImageContent::text(prompt_text)]),
        timestamp: now_ms(),
    })];
    let response = complete_simple_with_retries(
        stream_fn,
        model.clone(),
        crate::agent::llm::types::Context {
            system_prompt: Some(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: summarization_messages,
            tools: Vec::new(),
        },
        {
            let mut options = crate::agent::llm::types::SimpleStreamOptions::default();
            options.max_tokens = Some(2048);
            options
        },
        None,
        None,
    )
    .await;
    if response.stop_reason == StopReason::Aborted {
        return err(BranchSummaryError::new(
            BranchSummaryErrorCode::Aborted,
            response
                .error_message
                .unwrap_or_else(|| "Branch summary aborted".to_string()),
        ));
    }
    if response.stop_reason == StopReason::Error {
        return err(BranchSummaryError::new(
            BranchSummaryErrorCode::SummarizationFailed,
            format!(
                "Branch summary failed: {}",
                response
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string())
            ),
        ));
    }

    let mut summary = content_text_of_assistant(&response);
    summary = format!("{BRANCH_SUMMARY_PREAMBLE}{summary}");
    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    summary.push_str(&format_file_operations(&read_files, &modified_files));

    ok(BranchSummaryResult {
        summary: if summary.is_empty() {
            "No summary generated".to_string()
        } else {
            summary
        },
        usage: Some(response.usage),
        read_files,
        modified_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::{test_assistant, test_model};
    use crate::agent::harness::session::types::MessageEntry;
    use crate::agent::llm::types::{AssistantContent, StopReason};
    use std::sync::{Arc, Mutex};

    fn message_entry(seq: i64, parent: Option<&str>, message: AgentMessage) -> Entry {
        Entry::Message(MessageEntry {
            id: format!("e{seq}"),
            seq,
            parent_id: parent.map(str::to_string),
            timestamp: 0,
            message,
            terminate: None,
        })
    }

    fn scripted_stream_fn(text: &str) -> (StreamFn, Arc<Mutex<Vec<String>>>) {
        let texts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![text.to_string()]));
        let sink = texts.clone();
        let stream_fn: StreamFn = Arc::new(move |_model, _context, _options| {
            let sink = sink.clone();
            Box::pin(async move {
                let text = sink.lock().unwrap().first().cloned().unwrap_or_default();
                let final_message =
                    test_assistant(vec![AssistantContent::text(text)], StopReason::Stop);
                let (stream, writer) = crate::agent::llm::event_stream::event_stream();
                writer.push(crate::agent::llm::types::AssistantMessageEvent::Start {
                    partial: test_assistant(vec![], StopReason::Pending),
                });
                writer.push(crate::agent::llm::types::AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message: final_message.clone(),
                });
                writer.end(final_message);
                stream
            })
        });
        (stream_fn, texts)
    }

    #[test]
    fn prepare_branch_entries_skips_tool_results() {
        let user_message = AgentMessage::user_text("explore", 0);
        let tool_result = AgentMessage::Message(crate::agent::types::TypedMessage::ToolResult(
            crate::agent::llm::types::ToolResultMessage {
                role: "toolResult".to_string(),
                tool_call_id: "t".to_string(),
                tool_name: "read".to_string(),
                content: vec![TextOrImageContent::text("content")],
                details: None,
                usage: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 0,
            },
        ));
        let entries = vec![
            message_entry(1, None, user_message),
            message_entry(2, Some("e1"), tool_result),
        ];
        let preparation = prepare_branch_entries(&entries, 0);
        assert_eq!(preparation.messages.len(), 1);
        assert_eq!(preparation.messages[0].role_name(), "user");
    }

    #[test]
    fn prepare_branch_entries_respects_budget() {
        let mut entries = Vec::new();
        for seq in 1..=10 {
            let message = AgentMessage::user_text(&"x".repeat(4000), 0);
            entries.push(message_entry(seq, None, message));
        }
        // 预算 3000 → 大约前 3 条(每条约 1000 tokens)。
        let preparation = prepare_branch_entries(&entries, 3000);
        assert!(preparation.total_tokens <= 3000 + 1000);
        assert!(preparation.messages.len() <= 4);
    }

    #[tokio::test]
    async fn generate_branch_summary_prepends_preamble() {
        let (stream_fn, _texts) = scripted_stream_fn("branch findings");
        let entries = vec![message_entry(1, None, AgentMessage::user_text("hello", 0))];
        let result = generate_branch_summary(
            &entries,
            GenerateBranchSummaryOptions {
                stream_fn: &stream_fn,
                model: &test_model(),
                custom_instructions: None,
                replace_instructions: false,
                reserve_tokens: 16384,
            },
        )
        .await
        .unwrap();
        assert!(result
            .summary
            .starts_with("The user explored a different conversation branch"));
        assert!(result.summary.contains("branch findings"));
        assert!(result.usage.is_some());
    }

    #[tokio::test]
    async fn generate_branch_summary_empty_entries() {
        let (stream_fn, _texts) = scripted_stream_fn("unused");
        let result = generate_branch_summary(
            &[],
            GenerateBranchSummaryOptions {
                stream_fn: &stream_fn,
                model: &test_model(),
                custom_instructions: None,
                replace_instructions: false,
                reserve_tokens: 16384,
            },
        )
        .await
        .unwrap();
        assert_eq!(result.summary, "No content to summarize");
        assert!(result.usage.is_none());
    }
}
