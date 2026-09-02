//! 会话上下文构建:对齐 `packages/agent/src/harness/session/context.ts`。

use std::collections::HashMap;
use std::sync::Arc;

use super::types::{CompactionEntry, CustomEntry, Entry};
use crate::agent::harness::messages::{
    create_branch_summary_message, create_compaction_summary_message,
};
use crate::agent::types::AgentMessage;

/// 从一条分支路径派生的会话上下文(对齐 TS `SessionContext`)。
#[derive(Clone, Debug, Default)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    /// thinking level 字符串(与 TS 一致保留原样,缺省 "off")。
    pub thinking_level: String,
    pub model: Option<SessionModelRef>,
    pub active_tool_names: Option<Vec<String>>,
}

/// `{provider, modelId}` 引用(对齐 TS 内联对象)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionModelRef {
    pub provider: String,
    pub model_id: String,
}

/// 条目变换钩子(对齐 TS `ContextEntryTransform`)。
pub type ContextEntryTransform = Arc<dyn Fn(&[Entry]) -> Vec<Entry> + Send + Sync>;

/// custom 条目投影器(对齐 TS `CustomEntryContextMessageProjector`)。
pub type CustomEntryContextMessageProjector =
    Arc<dyn Fn(&CustomEntry, usize, &[Entry]) -> Vec<AgentMessage> + Send + Sync>;

/// 上下文构建选项(对齐 TS `SessionContextBuildOptions`)。
#[derive(Clone, Default)]
pub struct SessionContextBuildOptions {
    pub entry_transforms: Vec<ContextEntryTransform>,
    pub entry_projectors: HashMap<String, CustomEntryContextMessageProjector>,
}

fn derive_session_context_state(
    path_entries: &[Entry],
) -> (String, Option<SessionModelRef>, Option<Vec<String>>) {
    let mut thinking_level = "off".to_string();
    let mut model: Option<SessionModelRef> = None;
    let mut active_tool_names: Option<Vec<String>> = None;

    for entry in path_entries {
        match entry {
            Entry::ThinkingLevelChange(change) => {
                thinking_level = change.thinking_level.clone();
            }
            Entry::ModelChange(change) => {
                model = Some(SessionModelRef {
                    provider: change.provider.clone(),
                    model_id: change.model_id.clone(),
                });
            }
            Entry::Message(message_entry) => {
                if let crate::agent::types::AgentMessage::Message(
                    crate::agent::types::TypedMessage::Assistant(assistant),
                ) = &message_entry.message
                {
                    model = Some(SessionModelRef {
                        provider: assistant.provider.clone(),
                        model_id: assistant.model.clone(),
                    });
                }
            }
            Entry::ActiveToolsChange(change) => {
                active_tool_names = Some(change.active_tool_names.clone());
            }
            _ => {}
        }
    }

    (thinking_level, model, active_tool_names)
}

/// 默认条目变换:仅保留最近一次 compaction 及其之后的条目。
pub fn default_context_entry_transform(path_entries: &[Entry]) -> Vec<Entry> {
    let mut compaction: Option<&CompactionEntry> = None;
    let mut compaction_index: isize = -1;
    for (index, entry) in path_entries.iter().enumerate().rev() {
        if let Entry::Compaction(entry) = entry {
            compaction = Some(entry);
            compaction_index = index as isize;
            break;
        }
    }
    match compaction {
        None => path_entries.to_vec(),
        Some(_) => {
            let start = compaction_index as usize;
            let mut entries = Vec::with_capacity(path_entries.len() - start);
            entries.push(Entry::Compaction(compaction.unwrap().clone()));
            entries.extend(path_entries[start + 1..].iter().cloned());
            entries
        }
    }
}

/// 应用默认变换与自定义变换,得到参与上下文的条目列表。
pub fn build_context_entries(
    path_entries: &[Entry],
    options: &SessionContextBuildOptions,
) -> Vec<Entry> {
    let mut entries = default_context_entry_transform(path_entries);
    for transform in &options.entry_transforms {
        entries = transform(&entries);
    }
    entries
}

/// 单条目 → 上下文消息(对齐 TS `sessionEntryToContextMessages`)。
pub fn session_entry_to_context_messages(
    entry: &Entry,
    index: usize,
    entries: &[Entry],
    options: &SessionContextBuildOptions,
) -> Vec<AgentMessage> {
    match entry {
        Entry::Message(message_entry) => {
            if let crate::agent::types::AgentMessage::Message(
                crate::agent::types::TypedMessage::Assistant(assistant),
            ) = &message_entry.message
            {
                if assistant.stop_reason == crate::agent::llm::types::StopReason::Deferred {
                    return Vec::new();
                }
            }
            vec![message_entry.message.clone()]
        }
        Entry::Compaction(compaction) => {
            let mut messages = vec![create_compaction_summary_message(
                compaction.summary.clone(),
                compaction.tokens_before,
                compaction.timestamp,
            )];
            messages.extend(compaction.retained_tail.iter().cloned());
            messages
        }
        Entry::BranchSummary(branch_summary) => {
            if branch_summary.summary.is_empty() {
                Vec::new()
            } else {
                vec![create_branch_summary_message(
                    branch_summary.summary.clone(),
                    branch_summary.from_id.clone(),
                    branch_summary.timestamp,
                )]
            }
        }
        Entry::Custom(custom_entry) => options
            .entry_projectors
            .get(&custom_entry.custom_type)
            .map(|projector| projector(custom_entry, index, entries))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 从分支路径条目构建会话上下文(对齐 TS `buildSessionContext`)。
pub fn build_session_context(
    path_entries: &[Entry],
    options: &SessionContextBuildOptions,
) -> SessionContext {
    let (thinking_level, model, active_tool_names) = derive_session_context_state(path_entries);
    let context_entries = build_context_entries(path_entries, options);
    let mut messages = Vec::new();
    for (index, entry) in context_entries.iter().enumerate() {
        messages.extend(session_entry_to_context_messages(
            entry,
            index,
            &context_entries,
            options,
        ));
    }
    SessionContext {
        messages,
        thinking_level,
        model,
        active_tool_names,
    }
}
