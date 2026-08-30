//! lane 状态归约:对齐 `packages/agent/src/harness/reducer.ts`。
//!
//! 纯函数重建单 lane 的编排状态:无会话状态读取/写入;输入为有界的恢复切片。
//! 蓝本 DeferredHandle 依赖 pi-ai 的 deferred 字段(本复刻 AssistantMessage 未
//! 建模 deferred),相关校验为 no-op、`deferred` 恒为 `None`(见报告偏差)。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::llm::types::{StopReason, ToolCall};
use crate::agent::types::AgentMessage;

use crate::agent::harness::session::types::{
    Entry, LaneRecord, OperationIntent, OperationStartedRecord, ProvisionedEntry, QueueKind,
    StepAttemptRecord, StepKind, ToolStartedRecord, UsageCauseKind,
};

/// 恢复切片矛盾的机器可读类别(对齐 TS `RecordLogCorruptionReason`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordLogCorruptionReason {
    MultipleOpenOperations,
    UnknownOperation,
    RecordAfterFinish,
    NonConsecutiveAttempt,
    InvalidCompactionReason,
    QueueAfterAbort,
    InvalidQueueCancellation,
    InconsistentStep,
    ToolCallMismatch,
    DuplicateToolInvocation,
    ProvisionedEntryMismatch,
    InvalidDeferredHandle,
}

/// 恢复切片矛盾错误(单写者协议不可能产生的状态;对齐 TS `RecordLogCorruption`)。
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct RecordLogCorruption {
    pub reason: RecordLogCorruptionReason,
    pub message: String,
}

fn corrupt(reason: RecordLogCorruptionReason, message: impl Into<String>) -> RecordLogCorruption {
    RecordLogCorruption {
        reason,
        message: message.into(),
    }
}

/// 有界的 lane 恢复切片(对齐 TS `RecordLogSlice`)。
#[derive(Clone, Debug, Default)]
pub struct RecordLogSlice {
    pub lane: String,
    pub open_operations: Vec<OperationStartedRecord>,
    pub records: Vec<LaneRecord>,
    /// operation 拥有的条目 + 经 provisioned/引用 id 直接取回的条目。
    pub entries: Vec<Entry>,
}

/// 生效 lane 配置(对齐 TS `EffectiveLaneConfiguration`)。
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveLaneConfiguration {
    pub model: (String, String),
    /// thinking level 字符串(与 TS 一致保留原样)。
    pub thinking_level: String,
    pub active_tool_names: Vec<String>,
}

/// 终态失败(对齐 TS `TerminalFailureState`)。
#[derive(Clone, Debug)]
pub struct TerminalFailureState {
    pub entry_id: String,
    /// "step" | "deferred_fetch"。
    pub source: TerminalFailureSource,
    pub message: crate::agent::llm::types::AssistantMessage,
}

/// 终态失败来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalFailureSource {
    Step,
    DeferredFetch,
}

/// 一个工具批次的调用状态(对齐 TS `ToolBatchState.calls[]`)。
#[derive(Clone, Debug)]
pub struct ToolBatchCall {
    pub tool_index: usize,
    pub tool_call: ToolCall,
    pub started: Option<ToolStartedRecord>,
    pub result_exists: bool,
    pub terminate: Option<bool>,
}

/// 工具批次状态(对齐 TS `ToolBatchState`)。
#[derive(Clone, Debug)]
pub struct ToolBatchState {
    pub assistant_entry_id: String,
    pub calls: Vec<ToolBatchCall>,
    pub truncated: bool,
    pub unresolved: bool,
}

/// operation 内部状态(对齐 TS `LaneState["operation"]`)。
#[derive(Clone, Debug)]
pub struct LaneOperationState {
    pub id: String,
    pub kind: OperationKindTag,
    pub intent: OperationIntent,
    pub aborting: bool,
    pub step: Option<LaneStepState>,
    pub tool_batch: Option<ToolBatchState>,
    pub missing_initial_messages: Vec<ProvisionedEntry>,
    pub pending_steer: Vec<ProvisionedEntry>,
    pub pending_follow_up: Vec<ProvisionedEntry>,
    pub pending_writes: Vec<ProvisionedEntry>,
    /// 蓝本为 DeferredHandle;本复刻 AssistantMessage 不建模 deferred,恒 None。
    pub deferred: Option<DeferredHandle>,
    pub overflow_recovery_used: bool,
    pub newest_own: Option<NewestOwn>,
    pub targets: OperationTargets,
}

/// operation 种类标签。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationKindTag {
    Run,
    Compaction,
    Navigation,
}

/// 进行中 step 状态。
#[derive(Clone, Debug)]
pub struct LaneStepState {
    pub kind: StepKind,
    pub attempts: i64,
    pub result_entry_id: String,
    pub compaction_reason: Option<crate::agent::harness::session::types::CompactionReason>,
}

/// 最新自有条目摘要(对齐 TS `newestOwn`)。
#[derive(Clone, Debug)]
pub struct NewestOwn {
    pub entry_id: String,
    pub entry_type: &'static str,
    pub role: Option<String>,
    pub stop_reason: Option<StopReason>,
}

/// operation 目标完成标记。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationTargets {
    pub result: bool,
    pub summary: bool,
}

/// DeferredHandle 形状(蓝本 pi-ai 类型;deferred 未建模,保留类型以对齐契约)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
    pub provider: String,
    pub model_id: String,
    pub api: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_after_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 单 lane 状态(对齐 TS `LaneState`)。
#[derive(Clone, Debug)]
pub struct LaneState {
    pub lane: String,
    pub leaf_id: Option<String>,
    pub operation: Option<LaneOperationState>,
    pub pending_next_run: Vec<ProvisionedEntry>,
}

/// 归约输入(对齐 TS `LaneReductionInput`)。
#[derive(Clone, Debug)]
pub struct LaneReductionInput {
    pub lane: String,
    pub leaf_id: Option<String>,
    pub open_operations: Vec<OperationStartedRecord>,
    pub records: Vec<LaneRecord>,
    /// operation 拥有的条目 + 直接取回的条目。
    pub entries: Vec<Entry>,
    /// 开放 operation 追加的条目,时间序;空闲时为空。
    pub own_entries: Vec<Entry>,
    /// operation 锚点或空闲叶处的有界生效状态查询,时间序。
    pub configuration_entries: Vec<Entry>,
    /// 无持久化值时的 harness 回退。
    pub defaults: EffectiveLaneConfiguration,
}

/// 归约结果(对齐 TS `LaneReductionResult`)。
#[derive(Clone, Debug)]
pub struct LaneReductionResult {
    pub lane_state: LaneState,
    pub effective_configuration: EffectiveLaneConfiguration,
    pub terminal_failure: Option<TerminalFailureState>,
}

fn as_assistant(message: &AgentMessage) -> Option<&crate::agent::llm::types::AssistantMessage> {
    match message {
        AgentMessage::Message(crate::agent::types::TypedMessage::Assistant(assistant)) => {
            Some(assistant)
        }
        _ => None,
    }
}

/// Entry 载荷(去掉 parentId/seq/timestamp)与 provisioned 目标的深度相等
/// (对齐 TS `matchesProvisionedEntry` 经 `Guard.IsDeepEqual`)。
fn matches_provisioned_entry(entry: &Entry, target: &ProvisionedEntry) -> bool {
    &entry.to_provisioned() == target
}

fn validate_exact_provisioned_entry(
    entries_by_id: &HashMap<String, Entry>,
    target: &ProvisionedEntry,
) -> Result<(), RecordLogCorruption> {
    if let Some(entry) = entries_by_id.get(target.id()) {
        if !matches_provisioned_entry(entry, target) {
            return Err(corrupt(
                RecordLogCorruptionReason::ProvisionedEntryMismatch,
                format!(
                    "Provisioned entry {} exists with content different from its intent",
                    target.id()
                ),
            ));
        }
    }
    Ok(())
}

fn validate_result_entry(
    entries_by_id: &HashMap<String, Entry>,
    result_entry_id: &str,
    matches: impl Fn(&Entry) -> bool,
    description: &str,
) -> Result<(), RecordLogCorruption> {
    if let Some(entry) = entries_by_id.get(result_entry_id) {
        if !matches(entry) {
            return Err(corrupt(
                RecordLogCorruptionReason::ProvisionedEntryMismatch,
                format!(
                    "Provisioned {description} entry {result_entry_id} exists with different content"
                ),
            ));
        }
    }
    Ok(())
}

fn validate_attempt_reason(record: &StepAttemptRecord) -> Result<(), RecordLogCorruption> {
    match record.step {
        StepKind::Compaction => {
            if record.compaction_reason.is_none() {
                return Err(corrupt(
                    RecordLogCorruptionReason::InvalidCompactionReason,
                    format!("Compaction attempt {} has no valid compaction reason", record.id),
                ));
            }
        }
        _ => {
            if record.compaction_reason.is_some() {
                return Err(corrupt(
                    RecordLogCorruptionReason::InvalidCompactionReason,
                    format!("{} attempt {} has a compaction reason", step_name(record.step), record.id),
                ));
            }
        }
    }
    Ok(())
}

fn step_name(step: StepKind) -> &'static str {
    match step {
        StepKind::Assistant => "assistant",
        StepKind::Compaction => "compaction",
        StepKind::BranchSummary => "branch_summary",
    }
}

fn validate_attempt_sequence(
    record: &StepAttemptRecord,
    previous: Option<&StepAttemptRecord>,
    entries_by_id: &HashMap<String, Entry>,
) -> Result<(), RecordLogCorruption> {
    let previous_record = previous;
    let previous_result = previous_record.and_then(|record| entries_by_id.get(&record.result_entry_id));
    let continues_series = match previous_record {
        Some(previous_record) => previous_record.step == record.step
            && previous_result
                .map(|result| result.seq() >= record.seq)
                .unwrap_or(true),
        None => false,
    };
    let expected_attempt = if continues_series {
        previous_record.unwrap().attempt + 1
    } else {
        1
    };
    if record.attempt != expected_attempt {
        return Err(corrupt(
            RecordLogCorruptionReason::NonConsecutiveAttempt,
            format!(
                "{} attempt {} is {}; expected {}",
                step_name(record.step),
                record.id,
                record.attempt,
                expected_attempt
            ),
        ));
    }
    if !continues_series || record.step == StepKind::Assistant || previous_record.is_none() {
        return Ok(());
    }
    let previous_record = previous_record.unwrap();
    if record.result_entry_id != previous_record.result_entry_id {
        return Err(corrupt(
            RecordLogCorruptionReason::InconsistentStep,
            format!("{} attempts disagree on their result entry id", step_name(record.step)),
        ));
    }
    if record.compaction_reason != previous_record.compaction_reason {
        return Err(corrupt(
            RecordLogCorruptionReason::InconsistentStep,
            format!(
                "{} attempts disagree on their compaction reason",
                step_name(record.step)
            ),
        ));
    }
    Ok(())
}

fn validate_attempt_result(
    entries_by_id: &HashMap<String, Entry>,
    record: &StepAttemptRecord,
) -> Result<(), RecordLogCorruption> {
    match record.step {
        StepKind::Assistant => validate_result_entry(
            entries_by_id,
            &record.result_entry_id,
            |entry| {
                matches!(entry, Entry::Message(message_entry)
                    if as_assistant(&message_entry.message).is_some())
            },
            "assistant result",
        ),
        StepKind::Compaction => validate_result_entry(
            entries_by_id,
            &record.result_entry_id,
            |entry| matches!(entry, Entry::Compaction(_)),
            "compaction result",
        ),
        StepKind::BranchSummary => validate_result_entry(
            entries_by_id,
            &record.result_entry_id,
            |entry| matches!(entry, Entry::BranchSummary(_)),
            "branch-summary result",
        ),
    }
}

fn validate_tool_start(
    record: &ToolStartedRecord,
    entries_by_id: &HashMap<String, Entry>,
    invocations: &mut HashSet<(String, usize)>,
) -> Result<(), RecordLogCorruption> {
    let invocation = (record.assistant_entry_id.clone(), record.tool_index);
    if invocations.contains(&invocation) {
        return Err(corrupt(
            RecordLogCorruptionReason::DuplicateToolInvocation,
            format!(
                "Tool invocation {}:{} is duplicated",
                record.assistant_entry_id, record.tool_index
            ),
        ));
    }
    invocations.insert(invocation);

    let Some(Entry::Message(assistant_entry)) = entries_by_id.get(&record.assistant_entry_id) else {
        return Err(corrupt(
            RecordLogCorruptionReason::ToolCallMismatch,
            format!("Tool start {} does not reference an assistant entry", record.id),
        ));
    };
    let Some(assistant_message) = as_assistant(&assistant_entry.message) else {
        return Err(corrupt(
            RecordLogCorruptionReason::ToolCallMismatch,
            format!("Tool start {} does not reference an assistant entry", record.id),
        ));
    };
    let tool_calls: Vec<&ToolCall> = assistant_message
        .content
        .iter()
        .filter_map(|content| match content {
            crate::agent::llm::types::AssistantContent::ToolCall(tool_call) => Some(tool_call),
            _ => None,
        })
        .collect();
    let Some(tool_call) = tool_calls.get(record.tool_index) else {
        return Err(corrupt(
            RecordLogCorruptionReason::ToolCallMismatch,
            format!(
                "Tool start {} does not match its assistant tool-call ordinal",
                record.id
            ),
        ));
    };
    if tool_call.id != record.tool_call_id || tool_call.name != record.tool_name {
        return Err(corrupt(
            RecordLogCorruptionReason::ToolCallMismatch,
            format!(
                "Tool start {} does not match its assistant tool-call ordinal",
                record.id
            ),
        ));
    }

    validate_result_entry(
        entries_by_id,
        &record.result_entry_id,
        |entry| {
            matches!(entry, Entry::Message(message_entry)
                if message_entry.message.role_name() == "toolResult"
                && tool_result_field(&message_entry.message, "toolCallId") == Some(record.tool_call_id.as_str())
                && tool_result_field(&message_entry.message, "toolName") == Some(record.tool_name.as_str()))
        },
        "tool result",
    )
}

fn tool_result_field<'a>(message: &'a AgentMessage, field: &str) -> Option<&'a str> {
    match message {
        AgentMessage::Message(crate::agent::types::TypedMessage::ToolResult(result)) => match field {
            "toolCallId" => Some(&result.tool_call_id),
            "toolName" => Some(&result.tool_name),
            _ => None,
        },
        _ => None,
    }
}

fn validate_operation_result(
    entries_by_id: &HashMap<String, Entry>,
    record: &OperationStartedRecord,
) -> Result<(), RecordLogCorruption> {
    match &record.intent {
        OperationIntent::Run { initial_messages, .. } => {
            for target in initial_messages {
                validate_exact_provisioned_entry(entries_by_id, target)?;
            }
        }
        OperationIntent::Compaction { result_entry_id, .. } => {
            validate_result_entry(
                entries_by_id,
                result_entry_id,
                |entry| matches!(entry, Entry::Compaction(_)),
                "manual compaction",
            )?;
        }
        OperationIntent::Navigation { summary_entry_id, .. } => {
            if let Some(summary_entry_id) = summary_entry_id {
                validate_result_entry(
                    entries_by_id,
                    summary_entry_id,
                    |entry| matches!(entry, Entry::BranchSummary(_)),
                    "navigation summary",
                )?;
            }
        }
    }
    Ok(())
}

/// 校验有界 lane 恢复切片,不读/不改会话状态(对齐 TS `validateRecordLog`)。
pub fn validate_record_log(input: &RecordLogSlice) -> Result<(), RecordLogCorruption> {
    if input.open_operations.len() > 1 {
        return Err(corrupt(
            RecordLogCorruptionReason::MultipleOpenOperations,
            format!("Lane {} has at least two open operations", input.lane),
        ));
    }

    let mut entries_by_id: HashMap<String, Entry> = HashMap::new();
    for entry in &input.entries {
        entries_by_id.insert(entry.id().to_string(), entry.clone());
    }
    // deferred 句柄校验在本复刻中无对应字段(见模块注释),no-op。
    let mut starts: HashMap<String, OperationStartedRecord> = HashMap::new();
    let mut finished_at: HashMap<String, i64> = HashMap::new();
    let mut aborted_at: HashMap<String, i64> = HashMap::new();
    let mut queue_enqueues: HashMap<String, &LaneRecord> = HashMap::new();
    let mut latest_attempt: HashMap<String, StepAttemptRecord> = HashMap::new();
    let mut tool_invocations: HashSet<(String, usize)> = HashSet::new();
    let mut records: Vec<&LaneRecord> = input.records.iter().collect();
    records.sort_by_key(|record| record.seq());

    for record in records {
        if let LaneRecord::OperationStarted(operation) = record {
            starts.insert(operation.id.clone(), operation.clone());
            validate_operation_result(&entries_by_id, operation)?;
            continue;
        }

        if let Some(run_id) = record.operation_identity() {
            if !starts.contains_key(run_id) {
                return Err(corrupt(
                    RecordLogCorruptionReason::UnknownOperation,
                    format!("Record {} references unknown operation {}", record.id(), run_id),
                ));
            }
            if let Some(finish_seq) = finished_at.get(run_id) {
                if record.seq() > *finish_seq {
                    return Err(corrupt(
                        RecordLogCorruptionReason::RecordAfterFinish,
                        format!(
                            "Record {} follows the finish of operation {}",
                            record.id(),
                            run_id
                        ),
                    ));
                }
            }
        }

        match record {
            LaneRecord::OperationStarted(_) => unreachable!("handled above"),
            LaneRecord::OperationFinished(finished) => {
                finished_at.insert(finished.run_id.clone(), finished.seq);
            }
            LaneRecord::AbortRequested(abort) => {
                aborted_at.insert(abort.run_id.clone(), abort.seq);
            }
            LaneRecord::StepAttempt(step) => {
                validate_attempt_reason(step)?;
                let previous = latest_attempt.get(&step.run_id).cloned();
                validate_attempt_sequence(step, previous.as_ref(), &entries_by_id)?;
                validate_attempt_result(&entries_by_id, step)?;
                latest_attempt.insert(step.run_id.clone(), step.clone());
            }
            LaneRecord::ToolStarted(tool) => {
                validate_tool_start(tool, &entries_by_id, &mut tool_invocations)?;
            }
            LaneRecord::QueueEnqueued(enqueue) => {
                if enqueue.queue != QueueKind::NextRun {
                    if let Some(run_id) = &enqueue.run_id {
                        if let Some(abort_seq) = aborted_at.get(run_id) {
                            if enqueue.seq > *abort_seq {
                                return Err(corrupt(
                                    RecordLogCorruptionReason::QueueAfterAbort,
                                    format!(
                                        "{} item {} was enqueued after abort",
                                        queue_name(enqueue.queue),
                                        enqueue.target.id()
                                    ),
                                ));
                            }
                        }
                    }
                }
                queue_enqueues.insert(enqueue.target.id().to_string(), record);
                validate_exact_provisioned_entry(&entries_by_id, &enqueue.target)?;
            }
            LaneRecord::QueueCancelled(cancelled) => {
                let invalid = match queue_enqueues.get(&cancelled.entry_id) {
                    None => true,
                    Some(enqueue) => {
                        let LaneRecord::QueueEnqueued(enqueue) = enqueue else {
                            unreachable!("map only holds queue_enqueued records");
                        };
                        enqueue.seq >= cancelled.seq
                            || enqueue.run_id != cancelled.run_id
                            || entries_by_id.contains_key(&cancelled.entry_id)
                    }
                };
                if invalid {
                    return Err(corrupt(
                        RecordLogCorruptionReason::InvalidQueueCancellation,
                        format!(
                            "Queue cancellation {} has no pending matching enqueue",
                            cancelled.id
                        ),
                    ));
                }
            }
            LaneRecord::WriteDeferred(deferred) => {
                validate_exact_provisioned_entry(&entries_by_id, &deferred.target)?;
            }
            LaneRecord::Usage(_) => {}
        }
    }
    Ok(())
}

fn queue_name(queue: QueueKind) -> &'static str {
    match queue {
        QueueKind::Steer => "steer",
        QueueKind::FollowUp => "followUp",
        QueueKind::NextRun => "nextRun",
    }
}

fn by_sequence<T: Clone + seq_key::Seq>(mut values: Vec<T>) -> Vec<T> {
    values.sort_by_key(|value| value.seq());
    values
}

/// seq 键辅助(避免为引用写 trait)。
mod seq_key {
    pub trait Seq {
        fn seq(&self) -> i64;
    }

    impl Seq for crate::agent::harness::session::types::LaneRecord {
        fn seq(&self) -> i64 {
            crate::agent::harness::session::types::LaneRecord::seq(self)
        }
    }

    impl Seq for crate::agent::harness::session::types::Entry {
        fn seq(&self) -> i64 {
            crate::agent::harness::session::types::Entry::seq(self)
        }
    }
}

fn derive_effective_configuration(input: &LaneReductionInput) -> EffectiveLaneConfiguration {
    let mut configuration = input.defaults.clone();
    let mut entries_by_id: HashMap<String, Entry> = HashMap::new();
    for entry in input.configuration_entries.iter().chain(input.own_entries.iter()) {
        entries_by_id.insert(entry.id().to_string(), entry.clone());
    }

    let ordered = by_sequence(entries_by_id.into_values().collect());
    for entry in ordered {
        match &entry {
            Entry::ModelChange(change) => {
                configuration.model = (change.provider.clone(), change.model_id.clone());
            }
            Entry::ThinkingLevelChange(change) => {
                configuration.thinking_level = change.thinking_level.clone();
            }
            Entry::ActiveToolsChange(change) => {
                configuration.active_tool_names = change.active_tool_names.clone();
            }
            Entry::Message(message_entry) => {
                if let Some(assistant) = as_assistant(&message_entry.message) {
                    configuration.model = (assistant.provider.clone(), assistant.model.clone());
                }
            }
            _ => {}
        }
    }
    configuration
}

fn derive_newest_own(entry: Option<&Entry>) -> Option<NewestOwn> {
    let entry = entry?;
    Some(match entry {
        Entry::Message(message_entry) => {
            if let Some(assistant) = as_assistant(&message_entry.message) {
                NewestOwn {
                    entry_id: entry.id().to_string(),
                    entry_type: entry.entry_type(),
                    role: Some("assistant".to_string()),
                    stop_reason: Some(assistant.stop_reason),
                }
            } else {
                NewestOwn {
                    entry_id: entry.id().to_string(),
                    entry_type: entry.entry_type(),
                    role: Some(message_entry.message.role_name().to_string()),
                    stop_reason: None,
                }
            }
        }
        other => NewestOwn {
            entry_id: other.id().to_string(),
            entry_type: other.entry_type(),
            role: None,
            stop_reason: None,
        },
    })
}

fn derive_tool_batch(
    operation_id: &str,
    records: &[&LaneRecord],
    own_entries: &[Entry],
    entries_by_id: &HashMap<String, Entry>,
    deferred_write_ids: &HashSet<String>,
) -> Option<ToolBatchState> {
    let assistant_entry = own_entries.iter().rev().find(|entry| {
        matches!(entry, Entry::Message(message_entry)
            if as_assistant(&message_entry.message)
                .map(|assistant| assistant.content.iter().any(|content| matches!(content,
                    crate::agent::llm::types::AssistantContent::ToolCall(_))))
                .unwrap_or(false))
    });
    let Entry::Message(assistant_message_entry) = assistant_entry? else {
        return None;
    };
    let Some(assistant_message) = as_assistant(&assistant_message_entry.message) else {
        return None;
    };

    let tool_calls: Vec<ToolCall> = assistant_message
        .content
        .iter()
        .filter_map(|content| match content {
            crate::agent::llm::types::AssistantContent::ToolCall(tool_call) => Some(tool_call.clone()),
            _ => None,
        })
        .collect();
    let mut starts: HashMap<usize, ToolStartedRecord> = HashMap::new();
    for record in records {
        if let LaneRecord::ToolStarted(started) = record {
            if started.run_id == operation_id && started.assistant_entry_id == assistant_message_entry.id {
                starts.insert(started.tool_index, started.clone());
            }
        }
    }

    let mut calls = Vec::new();
    for (tool_index, tool_call) in tool_calls.into_iter().enumerate() {
        let started = starts.get(&tool_index).cloned();
        let started_result = started
            .as_ref()
            .and_then(|started| entries_by_id.get(&started.result_entry_id));
        let blocked_result = own_entries.iter().find(|entry| {
            entry.seq() > assistant_message_entry.seq
                && !deferred_write_ids.contains(entry.id())
                && matches!(entry, Entry::Message(message_entry)
                    if message_entry.message.role_name() == "toolResult"
                        && tool_result_field(&message_entry.message, "toolCallId") == Some(tool_call.id.as_str()))
        });
        let result = started_result.or(blocked_result);
        let terminate = match result {
            Some(Entry::Message(message_entry)) if message_entry.terminate == Some(true) => {
                Some(true)
            }
            _ => None,
        };
        calls.push(ToolBatchCall {
            tool_index,
            tool_call,
            started,
            result_exists: result.is_some(),
            terminate,
        });
    }

    let unresolved = calls.iter().any(|call| !call.result_exists);
    Some(ToolBatchState {
        assistant_entry_id: assistant_message_entry.id.clone(),
        calls,
        truncated: assistant_message.stop_reason == StopReason::Length,
        unresolved,
    })
}

/// 纯函数重建单 lane 的编排状态(对齐 TS `reduceLaneState`)。
pub fn reduce_lane_state(input: &LaneReductionInput) -> Result<LaneReductionResult, RecordLogCorruption> {
    let slice = RecordLogSlice {
        lane: input.lane.clone(),
        open_operations: input.open_operations.clone(),
        records: input.records.clone(),
        entries: input.entries.clone(),
    };
    validate_record_log(&slice)?;

    let records = by_sequence(input.records.clone());
    let own_entries = by_sequence(input.own_entries.clone());
    let mut entries_by_id: HashMap<String, Entry> = HashMap::new();
    for entry in input.entries.iter().chain(own_entries.iter()) {
        entries_by_id.insert(entry.id().to_string(), entry.clone());
    }
    let cancelled_queue_ids: HashSet<String> = records
        .iter()
        .filter_map(|record| match record {
            LaneRecord::QueueCancelled(cancelled) => Some(cancelled.entry_id.clone()),
            _ => None,
        })
        .collect();
    let pending_queue_records: Vec<crate::agent::harness::session::types::QueueEnqueuedRecord> = records
        .iter()
        .filter_map(|record| match record {
            LaneRecord::QueueEnqueued(enqueue)
                if !entries_by_id.contains_key(enqueue.target.id())
                    && !cancelled_queue_ids.contains(enqueue.target.id()) =>
            {
                Some(enqueue.clone())
            }
            _ => None,
        })
        .collect();
    let started = input.open_operations.first().cloned();
    let captured_initial_message_ids: HashSet<String> = started
        .as_ref()
        .filter(|operation| matches!(operation.intent, OperationIntent::Run { .. }))
        .map(|operation| match &operation.intent {
            OperationIntent::Run { initial_messages, .. } => initial_messages
                .iter()
                .map(ProvisionedEntry::id)
                .map(str::to_string)
                .collect(),
            _ => unreachable!(),
        })
        .unwrap_or_default();
    let pending_next_run: Vec<ProvisionedEntry> = pending_queue_records
        .iter()
        .filter(|record| {
            record.queue == QueueKind::NextRun
                && !captured_initial_message_ids.contains(record.target.id())
        })
        .map(|record| record.target.clone())
        .collect();
    let effective_configuration = derive_effective_configuration(input);

    let Some(started) = started else {
        return Ok(LaneReductionResult {
            lane_state: LaneState {
                lane: input.lane.clone(),
                leaf_id: input.leaf_id.clone(),
                operation: None,
                pending_next_run,
            },
            effective_configuration,
            terminal_failure: None,
        });
    };

    let operation_records: Vec<&LaneRecord> = records
        .iter()
        .filter(|record| match record {
            LaneRecord::OperationStarted(operation) => operation.id == started.id,
            _ => record.operation_identity() == Some(started.id.as_str()),
        })
        .collect();
    let aborting = operation_records
        .iter()
        .any(|record| matches!(record, LaneRecord::AbortRequested(_)));
    let pending_steer: Vec<ProvisionedEntry> = if aborting {
        Vec::new()
    } else {
        pending_queue_records
            .iter()
            .filter(|record| record.queue == QueueKind::Steer && record.run_id.as_deref() == Some(started.id.as_str()))
            .map(|record| record.target.clone())
            .collect()
    };
    let pending_follow_up: Vec<ProvisionedEntry> = if aborting {
        Vec::new()
    } else {
        pending_queue_records
            .iter()
            .filter(|record| record.queue == QueueKind::FollowUp && record.run_id.as_deref() == Some(started.id.as_str()))
            .map(|record| record.target.clone())
            .collect()
    };
    let pending_writes: Vec<ProvisionedEntry> = operation_records
        .iter()
        .filter_map(|record| match record {
            LaneRecord::WriteDeferred(deferred) if !entries_by_id.contains_key(deferred.target.id()) => {
                Some(deferred.target.clone())
            }
            _ => None,
        })
        .collect();
    let missing_initial_messages: Vec<ProvisionedEntry> = match &started.intent {
        OperationIntent::Run { initial_messages, .. } => initial_messages
            .iter()
            .filter(|target| !entries_by_id.contains_key(target.id()))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };

    let newest_attempt: Option<&LaneRecord> = operation_records
        .iter()
        .rev()
        .find(|record| matches!(record, LaneRecord::StepAttempt(_)))
        .copied();
    let step: Option<LaneStepState> = match newest_attempt {
        Some(LaneRecord::StepAttempt(attempt)) if !entries_by_id.contains_key(&attempt.result_entry_id) => {
            Some(LaneStepState {
                kind: attempt.step,
                attempts: attempt.attempt,
                result_entry_id: attempt.result_entry_id.clone(),
                compaction_reason: if attempt.step == StepKind::Compaction {
                    attempt.compaction_reason
                } else {
                    None
                },
            })
        }
        _ => None,
    };

    let mut consumed_input_ids: HashSet<String> = HashSet::new();
    if let OperationIntent::Run { initial_messages, .. } = &started.intent {
        for target in initial_messages {
            consumed_input_ids.insert(target.id().to_string());
        }
    }
    for record in &operation_records {
        if let LaneRecord::QueueEnqueued(enqueue) = record {
            if enqueue.queue != QueueKind::NextRun {
                consumed_input_ids.insert(enqueue.target.id().to_string());
            }
        }
    }
    let mut newest_consumed_input_sequence = i64::MIN;
    for id in &consumed_input_ids {
        if let Some(Entry::Message(entry)) = entries_by_id.get(id) {
            newest_consumed_input_sequence = newest_consumed_input_sequence.max(entry.seq);
        }
    }
    let overflow_recovery_used = operation_records.iter().any(|record| {
        matches!(record, LaneRecord::StepAttempt(step)
            if step.step == StepKind::Compaction
                && step.compaction_reason == Some(crate::agent::harness::session::types::CompactionReason::Overflow)
                && step.seq > newest_consumed_input_sequence)
    });

    let newest_own_entry = own_entries.last();
    let newest_own = derive_newest_own(newest_own_entry);
    // deferred:AssistantMessage 未建模 deferred(蓝本 DeferredHandle),恒 None。
    let deferred: Option<DeferredHandle> = None;
    let mut targets = OperationTargets::default();
    match &started.intent {
        OperationIntent::Compaction { result_entry_id, .. } => {
            targets.result = entries_by_id.contains_key(result_entry_id);
        }
        OperationIntent::Navigation { summary_entry_id, .. } => {
            if let Some(summary_entry_id) = summary_entry_id {
                targets.summary = entries_by_id.contains_key(summary_entry_id);
            }
        }
        OperationIntent::Run { .. } => {}
    }

    let deferred_write_ids: HashSet<String> = operation_records
        .iter()
        .filter_map(|record| match record {
            LaneRecord::WriteDeferred(deferred) => Some(deferred.target.id().to_string()),
            _ => None,
        })
        .collect();

    let mut terminal_failure: Option<TerminalFailureState> = None;
    if let Some(Entry::Message(message_entry)) = newest_own_entry {
        if let Some(assistant) = as_assistant(&message_entry.message) {
            if assistant.stop_reason == StopReason::Error
                && !deferred_write_ids.contains(&message_entry.id)
            {
                let produced_by_step = operation_records.iter().any(|record| {
                    matches!(record, LaneRecord::StepAttempt(step)
                        if step.result_entry_id == message_entry.id)
                });
                let previous_own_entry = own_entries.get(own_entries.len().saturating_sub(2));
                let produced_by_deferred_fetch = operation_records.iter().any(|record| {
                    matches!(record, LaneRecord::Usage(usage)
                        if usage.cause == UsageCauseKind::DeferredFetch
                            && usage.entry_id.as_deref() == Some(message_entry.id.as_str()))
                }) || matches!(previous_own_entry, Some(Entry::Message(previous))
                    if as_assistant(&previous.message)
                        .map(|assistant| assistant.stop_reason == StopReason::Deferred)
                        .unwrap_or(false));
                if produced_by_step || produced_by_deferred_fetch {
                    terminal_failure = Some(TerminalFailureState {
                        entry_id: message_entry.id.clone(),
                        source: if produced_by_step {
                            TerminalFailureSource::Step
                        } else {
                            TerminalFailureSource::DeferredFetch
                        },
                        message: assistant.clone(),
                    });
                }
            }
        }
    }

    let kind = match &started.intent {
        OperationIntent::Run { .. } => OperationKindTag::Run,
        OperationIntent::Compaction { .. } => OperationKindTag::Compaction,
        OperationIntent::Navigation { .. } => OperationKindTag::Navigation,
    };

    Ok(LaneReductionResult {
        lane_state: LaneState {
            lane: input.lane.clone(),
            leaf_id: input.leaf_id.clone(),
            operation: Some(LaneOperationState {
                id: started.id.clone(),
                kind,
                intent: started.intent.clone(),
                aborting,
                step,
                tool_batch: derive_tool_batch(
                    &started.id,
                    &operation_records,
                    &own_entries,
                    &entries_by_id,
                    &deferred_write_ids,
                ),
                missing_initial_messages,
                pending_steer,
                pending_follow_up,
                pending_writes,
                deferred,
                overflow_recovery_used,
                newest_own,
                targets,
            }),
            pending_next_run,
        },
        effective_configuration,
        terminal_failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::testing::{test_assistant, test_tool_call};
    use crate::agent::harness::session::types::{
        MessageEntry, OperationStartedRecord, QueueEnqueuedRecord, ProvisionedCustomEntry,
        ProvisionedEntry, UsageCauseKind, UsageRecord,
    };
    use crate::agent::llm::types::AssistantContent;
    use serde_json::json;

    fn defaults() -> EffectiveLaneConfiguration {
        EffectiveLaneConfiguration {
            model: ("custom".to_string(), "test-model".to_string()),
            thinking_level: "off".to_string(),
            active_tool_names: vec![],
        }
    }

    fn idle_input(lane: &str) -> LaneReductionInput {
        LaneReductionInput {
            lane: lane.to_string(),
            leaf_id: None,
            open_operations: vec![],
            records: vec![],
            entries: vec![],
            own_entries: vec![],
            configuration_entries: vec![],
            defaults: defaults(),
        }
    }

    #[test]
    fn idle_lane_reduces_to_null_operation() {
        let result = reduce_lane_state(&idle_input("main")).unwrap();
        assert_eq!(result.lane_state.lane, "main");
        assert!(result.lane_state.operation.is_none());
        assert!(result.lane_state.pending_next_run.is_empty());
        assert!(result.terminal_failure.is_none());
        assert_eq!(result.effective_configuration.model.1, "test-model");
    }

    #[test]
    fn multiple_open_operations_is_corruption() {
        let mut input = idle_input("main");
        input.open_operations = vec![
            OperationStartedRecord {
                id: "op-1".into(),
                seq: 1,
                lane: "main".into(),
                timestamp: 0,
                source_leaf_id: None,
                intent: OperationIntent::Run {
                    original_prompt: vec![],
                    initial_messages: vec![],
                    system_prompt_override: None,
                    resume_data: None,
                },
            },
            OperationStartedRecord {
                id: "op-2".into(),
                seq: 2,
                lane: "main".into(),
                timestamp: 0,
                source_leaf_id: None,
                intent: OperationIntent::Run {
                    original_prompt: vec![],
                    initial_messages: vec![],
                    system_prompt_override: None,
                    resume_data: None,
                },
            },
        ];
        let error = reduce_lane_state(&input).unwrap_err();
        assert_eq!(error.reason, RecordLogCorruptionReason::MultipleOpenOperations);
    }

    #[test]
    fn run_operation_with_initial_messages_reduces() {
        let provisioned = ProvisionedEntry::Custom(ProvisionedCustomEntry {
            id: "in-1".into(),
            custom_type: "note".into(),
            data: None,
        });
        let entry = Entry::Custom(crate::agent::harness::session::types::CustomEntry {
            id: "in-1".into(),
            seq: 2,
            parent_id: None,
            timestamp: 5,
            custom_type: "note".into(),
            data: None,
        });
        let mut input = idle_input("main");
        input.leaf_id = Some("in-1".to_string());
        input.open_operations = vec![OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![provisioned],
                system_prompt_override: None,
                resume_data: None,
            },
        }];
        input.own_entries = vec![entry.clone()];
        let result = reduce_lane_state(&input).unwrap();
        let operation = result.lane_state.operation.as_ref().unwrap();
        assert_eq!(operation.id, "op-1");
        assert!(matches!(operation.kind, OperationKindTag::Run));
        assert!(!operation.aborting);
        assert!(operation.missing_initial_messages.is_empty());
        assert_eq!(operation.newest_own.as_ref().unwrap().entry_id, "in-1");
        assert!(operation.tool_batch.is_none());
    }

    #[test]
    fn provisioned_entry_mismatch_is_corruption() {
        let provisioned = ProvisionedEntry::Custom(ProvisionedCustomEntry {
            id: "in-1".into(),
            custom_type: "note".into(),
            data: Some(json!({"a": 1})),
        });
        let entry = Entry::Custom(crate::agent::harness::session::types::CustomEntry {
            id: "in-1".into(),
            seq: 2,
            parent_id: None,
            timestamp: 5,
            custom_type: "note".into(),
            data: Some(json!({"a": 2})),
        });
        let mut input = idle_input("main");
        input.open_operations = vec![OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![provisioned],
                system_prompt_override: None,
                resume_data: None,
            },
        }];
        input.entries = vec![entry];
        input.records = vec![LaneRecord::OperationStarted(OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![ProvisionedEntry::Custom(ProvisionedCustomEntry {
                    id: "in-1".into(),
                    custom_type: "note".into(),
                    data: Some(json!({"a": 1})),
                })],
                system_prompt_override: None,
                resume_data: None,
            },
        })];
        let error = reduce_lane_state(&input).unwrap_err();
        assert_eq!(
            error.reason,
            RecordLogCorruptionReason::ProvisionedEntryMismatch
        );
    }

    #[test]
    fn steering_queue_and_abort() {
        let steer_target = ProvisionedEntry::Custom(ProvisionedCustomEntry {
            id: "q-1".into(),
            custom_type: "steer".into(),
            data: None,
        });
        let mut input = idle_input("main");
        input.open_operations = vec![OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![],
                system_prompt_override: None,
                resume_data: None,
            },
        }];
        input.records = vec![
            LaneRecord::OperationStarted(OperationStartedRecord {
                id: "op-1".into(),
                seq: 1,
                lane: "main".into(),
                timestamp: 0,
                source_leaf_id: None,
                intent: OperationIntent::Run {
                    original_prompt: vec![],
                    initial_messages: vec![],
                    system_prompt_override: None,
                    resume_data: None,
                },
            }),
            LaneRecord::QueueEnqueued(QueueEnqueuedRecord {
                id: "r-1".into(),
                seq: 2,
                lane: "main".into(),
                timestamp: 0,
                queue: QueueKind::Steer,
                run_id: Some("op-1".into()),
                target: steer_target.clone(),
            }),
        ];
        let result = reduce_lane_state(&input).unwrap();
        let operation = result.lane_state.operation.as_ref().unwrap();
        assert_eq!(operation.pending_steer.len(), 1);
        assert!(!operation.aborting);

        input.records.push(LaneRecord::AbortRequested(
            crate::agent::harness::session::types::AbortRequestedRecord {
                id: "r-2".into(),
                seq: 3,
                lane: "main".into(),
                timestamp: 0,
                run_id: "op-1".into(),
            },
        ));
        let result = reduce_lane_state(&input).unwrap();
        let operation = result.lane_state.operation.as_ref().unwrap();
        assert!(operation.aborting);
        assert!(operation.pending_steer.is_empty());
    }

    #[test]
    fn queue_after_abort_is_corruption() {
        let steer_target = ProvisionedEntry::Custom(ProvisionedCustomEntry {
            id: "q-1".into(),
            custom_type: "steer".into(),
            data: None,
        });
        let mut input = idle_input("main");
        input.open_operations = vec![OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![],
                system_prompt_override: None,
                resume_data: None,
            },
        }];
        input.records = vec![
            LaneRecord::OperationStarted(OperationStartedRecord {
                id: "op-1".into(),
                seq: 1,
                lane: "main".into(),
                timestamp: 0,
                source_leaf_id: None,
                intent: OperationIntent::Run {
                    original_prompt: vec![],
                    initial_messages: vec![],
                    system_prompt_override: None,
                    resume_data: None,
                },
            }),
            LaneRecord::AbortRequested(crate::agent::harness::session::types::AbortRequestedRecord {
                id: "r-abort".into(),
                seq: 2,
                lane: "main".into(),
                timestamp: 0,
                run_id: "op-1".into(),
            }),
            LaneRecord::QueueEnqueued(QueueEnqueuedRecord {
                id: "r-3".into(),
                seq: 3,
                lane: "main".into(),
                timestamp: 0,
                queue: QueueKind::Steer,
                run_id: Some("op-1".into()),
                target: steer_target,
            }),
        ];
        let error = reduce_lane_state(&input).unwrap_err();
        assert_eq!(error.reason, RecordLogCorruptionReason::QueueAfterAbort);
    }

    #[test]
    fn error_stop_assistant_produces_terminal_failure() {
        let mut failed = test_assistant(vec![], StopReason::Error);
        failed.error_message = Some("boom".to_string());
        let entry = Entry::Message(MessageEntry {
            id: "m-1".into(),
            seq: 2,
            parent_id: None,
            timestamp: 0,
            message: AgentMessage::Message(crate::agent::types::TypedMessage::Assistant(
                failed.clone(),
            )),
            terminate: None,
        });
        let mut input = idle_input("main");
        input.open_operations = vec![OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![],
                system_prompt_override: None,
                resume_data: None,
            },
        }];
        input.own_entries = vec![entry];
        input.records = vec![
            LaneRecord::OperationStarted(OperationStartedRecord {
                id: "op-1".into(),
                seq: 1,
                lane: "main".into(),
                timestamp: 0,
                source_leaf_id: None,
                intent: OperationIntent::Run {
                    original_prompt: vec![],
                    initial_messages: vec![],
                    system_prompt_override: None,
                    resume_data: None,
                },
            }),
            LaneRecord::StepAttempt(crate::agent::harness::session::types::StepAttemptRecord {
                id: "r-step".into(),
                seq: 2,
                lane: "main".into(),
                timestamp: 0,
                run_id: "op-1".into(),
                step: crate::agent::harness::session::types::StepKind::Assistant,
                attempt: 1,
                result_entry_id: "m-1".into(),
                compaction_reason: None,
            }),
        ];
        let result = reduce_lane_state(&input).unwrap();
        let failure = result.terminal_failure.unwrap();
        assert_eq!(failure.entry_id, "m-1");
        assert_eq!(failure.source, TerminalFailureSource::Step);
        assert_eq!(failure.message.error_message.as_deref(), Some("boom"));
    }

    #[test]
    fn derive_tool_batch_tracks_unresolved_calls() {
        let assistant = test_assistant(
            vec![
                AssistantContent::ToolCall(test_tool_call("c1", "read", json!({"path": "/a"}))),
                AssistantContent::ToolCall(test_tool_call("c2", "bash", json!({"command": "ls"}))),
            ],
            StopReason::ToolUse,
        );
        let assistant_entry = Entry::Message(MessageEntry {
            id: "m-1".into(),
            seq: 2,
            parent_id: None,
            timestamp: 0,
            message: AgentMessage::Message(crate::agent::types::TypedMessage::Assistant(assistant)),
            terminate: None,
        });
        let mut input = idle_input("main");
        input.open_operations = vec![OperationStartedRecord {
            id: "op-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            source_leaf_id: None,
            intent: OperationIntent::Run {
                original_prompt: vec![],
                initial_messages: vec![],
                system_prompt_override: None,
                resume_data: None,
            },
        }];
        input.own_entries = vec![assistant_entry];
        let result = reduce_lane_state(&input).unwrap();
        let operation = result.lane_state.operation.as_ref().unwrap();
        let batch = operation.tool_batch.as_ref().unwrap();
        assert_eq!(batch.calls.len(), 2);
        assert!(batch.unresolved);
        assert!(!batch.truncated);
    }

    #[test]
    fn configuration_entries_drive_effective_configuration() {
        let mut input = idle_input("main");
        input.configuration_entries = vec![Entry::ModelChange(
            crate::agent::harness::session::types::ModelChangeEntry {
                id: "mc-1".into(),
                seq: 1,
                parent_id: None,
                timestamp: 0,
                provider: "openai".into(),
                model_id: "gpt-5".into(),
            },
        )];
        input.own_entries = vec![Entry::ThinkingLevelChange(
            crate::agent::harness::session::types::ThinkingLevelEntry {
                id: "tl-1".into(),
                seq: 2,
                parent_id: None,
                timestamp: 0,
                thinking_level: "high".into(),
            },
        )];
        let result = reduce_lane_state(&input).unwrap();
        assert_eq!(result.effective_configuration.model, ("openai".to_string(), "gpt-5".to_string()));
        assert_eq!(result.effective_configuration.thinking_level, "high");
    }

    #[test]
    fn usage_record_shape_round_trips() {
        let record = UsageRecord {
            id: "u-1".into(),
            seq: 1,
            lane: "main".into(),
            timestamp: 0,
            usage: crate::agent::llm::types::Usage::zero(),
            cause: UsageCauseKind::Tool,
            run_id: Some("op-1".into()),
            entry_id: Some("m-1".into()),
            attempt: None,
            stop_reason: None,
            tool_call_id: Some("c1".into()),
            details: None,
        };
        let value = serde_json::to_value(&crate::agent::harness::session::types::LaneRecord::Usage(record.clone())).unwrap();
        assert_eq!(value["type"], "usage");
        assert_eq!(value["cause"], "tool");
        assert_eq!(value["toolCallId"], "c1");
    }
}
