//! session 类型:对齐 `packages/agent/src/harness/session/types.ts`。
//!
//! 序列化形状与 TS JSONL 存储完全兼容(`type`/`kind`/`cause` 内部 tag,字段
//! camelCase)。TS 的联合展开(step_attempt 的 compactionReason、usage 的四种
//! cause、queue_enqueued 的 runId 缺省)以 `Option` 扁平字段承载,不变式由
//! `reducer` 校验,与 TS 的运行时检查等价(见报告偏差说明)。

use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::agent::llm::types::{StopReason, Usage};
use crate::agent::types::AgentMessage;

/// TS `JsonValue` 对应物:直接复用 `serde_json::Value`(数值/字符串/布尔/null/
/// 数组/对象,与蓝本约束一致)。
pub type JsonValue = Value;

/// `StopReason` 去掉 `pending` 的会话侧口径(蓝本额外并回 `deferred`,而
/// `StopReason` 本身已含 `deferred`,故直接别名;`pending` 仅存在于流式中间态,
/// 不应落库,由校验逻辑保证)。
pub type SessionStopReason = StopReason;

/// id 生成器(对齐 TS `IdGenerator`;缺省实现为 UUIDv7,见 harness::uuid)。
pub trait IdGenerator: Send + Sync {
    fn next(&self) -> String;
}

/// 缺省 id 生成器:UUIDv7。
#[derive(Clone, Copy, Debug, Default)]
pub struct UuidIdGenerator;

impl IdGenerator for UuidIdGenerator {
    fn next(&self) -> String {
        crate::agent::harness::uuid::uuid_v7()
    }
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

/// role = "message" 的消息条目(载荷与消息本身,JSON 与 TS 完全一致)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub message: AgentMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChangeEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub provider: String,
    pub model_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub thinking_level: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveToolsEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub active_tool_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub summary: String,
    pub retained_tail: Vec<AgentMessage>,
    pub tokens_before: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub from_id: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEntry {
    pub id: String,
    pub seq: i64,
    pub parent_id: Option<String>,
    pub timestamp: i64,
    pub custom_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 会话树条目(对齐 TS `Entry`;`type` 为内部 tag,值与蓝本一致)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Entry {
    #[serde(rename = "message")]
    Message(MessageEntry),
    #[serde(rename = "model_change")]
    ModelChange(ModelChangeEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(ThinkingLevelEntry),
    #[serde(rename = "active_tools_change")]
    ActiveToolsChange(ActiveToolsEntry),
    #[serde(rename = "compaction")]
    Compaction(CompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(BranchSummaryEntry),
    #[serde(rename = "custom")]
    Custom(CustomEntry),
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Entry::Message(e) => &e.id,
            Entry::ModelChange(e) => &e.id,
            Entry::ThinkingLevelChange(e) => &e.id,
            Entry::ActiveToolsChange(e) => &e.id,
            Entry::Compaction(e) => &e.id,
            Entry::BranchSummary(e) => &e.id,
            Entry::Custom(e) => &e.id,
        }
    }

    pub fn seq(&self) -> i64 {
        match self {
            Entry::Message(e) => e.seq,
            Entry::ModelChange(e) => e.seq,
            Entry::ThinkingLevelChange(e) => e.seq,
            Entry::ActiveToolsChange(e) => e.seq,
            Entry::Compaction(e) => e.seq,
            Entry::BranchSummary(e) => e.seq,
            Entry::Custom(e) => e.seq,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Entry::Message(e) => e.parent_id.as_deref(),
            Entry::ModelChange(e) => e.parent_id.as_deref(),
            Entry::ThinkingLevelChange(e) => e.parent_id.as_deref(),
            Entry::ActiveToolsChange(e) => e.parent_id.as_deref(),
            Entry::Compaction(e) => e.parent_id.as_deref(),
            Entry::BranchSummary(e) => e.parent_id.as_deref(),
            Entry::Custom(e) => e.parent_id.as_deref(),
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            Entry::Message(e) => e.timestamp,
            Entry::ModelChange(e) => e.timestamp,
            Entry::ThinkingLevelChange(e) => e.timestamp,
            Entry::ActiveToolsChange(e) => e.timestamp,
            Entry::Compaction(e) => e.timestamp,
            Entry::BranchSummary(e) => e.timestamp,
            Entry::Custom(e) => e.timestamp,
        }
    }

    /// 条目判别名(对齐 TS `Entry["type"]`)。
    pub fn entry_type(&self) -> &'static str {
        match self {
            Entry::Message(_) => "message",
            Entry::ModelChange(_) => "model_change",
            Entry::ThinkingLevelChange(_) => "thinking_level_change",
            Entry::ActiveToolsChange(_) => "active_tools_change",
            Entry::Compaction(_) => "compaction",
            Entry::BranchSummary(_) => "branch_summary",
            Entry::Custom(_) => "custom",
        }
    }

    /// 去掉 `parentId`/`seq`/`timestamp` 的载荷视图(对齐 TS `ProvisionedEntry`)。
    pub fn to_provisioned(&self) -> ProvisionedEntry {
        match self {
            Entry::Message(e) => ProvisionedEntry::Message(ProvisionedMessageEntry {
                id: e.id.clone(),
                message: e.message.clone(),
                terminate: e.terminate,
            }),
            Entry::ModelChange(e) => ProvisionedEntry::ModelChange(ProvisionedModelChangeEntry {
                id: e.id.clone(),
                provider: e.provider.clone(),
                model_id: e.model_id.clone(),
            }),
            Entry::ThinkingLevelChange(e) => {
                ProvisionedEntry::ThinkingLevelChange(ProvisionedThinkingLevelEntry {
                    id: e.id.clone(),
                    thinking_level: e.thinking_level.clone(),
                })
            }
            Entry::ActiveToolsChange(e) => {
                ProvisionedEntry::ActiveToolsChange(ProvisionedActiveToolsEntry {
                    id: e.id.clone(),
                    active_tool_names: e.active_tool_names.clone(),
                })
            }
            Entry::Compaction(e) => ProvisionedEntry::Compaction(ProvisionedCompactionEntry {
                id: e.id.clone(),
                summary: e.summary.clone(),
                retained_tail: e.retained_tail.clone(),
                tokens_before: e.tokens_before,
                details: e.details.clone(),
                usage: e.usage.clone(),
            }),
            Entry::BranchSummary(e) => ProvisionedEntry::BranchSummary(ProvisionedBranchSummaryEntry {
                id: e.id.clone(),
                from_id: e.from_id.clone(),
                summary: e.summary.clone(),
                details: e.details.clone(),
                usage: e.usage.clone(),
            }),
            Entry::Custom(e) => ProvisionedEntry::Custom(ProvisionedCustomEntry {
                id: e.id.clone(),
                custom_type: e.custom_type.clone(),
                data: e.data.clone(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// ProvisionedEntry(写入前载荷,不带 parentId/seq/timestamp)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedMessageEntry {
    pub id: String,
    pub message: AgentMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedModelChangeEntry {
    pub id: String,
    pub provider: String,
    pub model_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedThinkingLevelEntry {
    pub id: String,
    pub thinking_level: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedActiveToolsEntry {
    pub id: String,
    pub active_tool_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedCompactionEntry {
    pub id: String,
    pub summary: String,
    pub retained_tail: Vec<AgentMessage>,
    pub tokens_before: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedBranchSummaryEntry {
    pub id: String,
    pub from_id: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionedCustomEntry {
    pub id: String,
    pub custom_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// `Omit<Entry, "parentId" | "seq" | "timestamp">`(对齐 TS `ProvisionedEntry`)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProvisionedEntry {
    #[serde(rename = "message")]
    Message(ProvisionedMessageEntry),
    #[serde(rename = "model_change")]
    ModelChange(ProvisionedModelChangeEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(ProvisionedThinkingLevelEntry),
    #[serde(rename = "active_tools_change")]
    ActiveToolsChange(ProvisionedActiveToolsEntry),
    #[serde(rename = "compaction")]
    Compaction(ProvisionedCompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(ProvisionedBranchSummaryEntry),
    #[serde(rename = "custom")]
    Custom(ProvisionedCustomEntry),
}

impl ProvisionedEntry {
    pub fn id(&self) -> &str {
        match self {
            ProvisionedEntry::Message(e) => &e.id,
            ProvisionedEntry::ModelChange(e) => &e.id,
            ProvisionedEntry::ThinkingLevelChange(e) => &e.id,
            ProvisionedEntry::ActiveToolsChange(e) => &e.id,
            ProvisionedEntry::Compaction(e) => &e.id,
            ProvisionedEntry::BranchSummary(e) => &e.id,
            ProvisionedEntry::Custom(e) => &e.id,
        }
    }
}

// ---------------------------------------------------------------------------
// LaneRecord
// ---------------------------------------------------------------------------

/// operation_started 的 intent(对齐 TS `OperationStartedRecord["intent"]`)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum OperationIntent {
    #[serde(rename = "run")]
    Run {
        /// before_run 前的归一化调用输入(挂起恢复与 before_resume 需要)。
        original_prompt: Vec<AgentMessage>,
        /// nextRun 捕获项 → prompt → before_run 注入。
        initial_messages: Vec<ProvisionedEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt_override: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_data: Option<serde_json::Map<String, Value>>,
    },
    #[serde(rename = "compaction")]
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
        result_entry_id: String,
    },
    #[serde(rename = "navigation")]
    Navigation {
        target_id: Option<String>,
        summarize: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_entry_id: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStartedRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub source_leaf_id: Option<String>,
    pub intent: OperationIntent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortRequestedRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub run_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationFinishedRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub run_id: String,
    pub outcome: OperationOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::agent::harness::errors::OperationError>,
}

/// operation 结束结果(对齐 TS `"completed" | "aborted" | "failed" | "declined"`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationOutcome {
    Completed,
    Aborted,
    Failed,
    Declined,
}

/// compaction 触发原因(对齐 TS `CompactionReason`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepAttemptRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub run_id: String,
    /// `"assistant" | "compaction" | "branch_summary"`。
    pub step: StepKind,
    pub attempt: i64,
    pub result_entry_id: String,
    /// compaction 尝试持久化触发原因;其他 step 必须缺省(reducer 校验)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_reason: Option<CompactionReason>,
}

/// 可重试 step 种类(对齐 TS 字面量)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Assistant,
    Compaction,
    BranchSummary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStartedRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub run_id: String,
    pub assistant_entry_id: String,
    pub tool_index: usize,
    pub tool_call_id: String,
    pub tool_name: String,
    pub effective_args: serde_json::Map<String, Value>,
    pub result_entry_id: String,
    /// `"never" | "safe"`。
    pub replay: ToolReplay,
}

/// 工具回放策略(对齐 TS 字面量)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolReplay {
    Never,
    Safe,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEnqueuedRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    /// `"steer" | "followUp" | "nextRun"`。
    pub queue: QueueKind,
    /// steer/followUp 必填;nextRun 缺省(reducer 校验)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub target: ProvisionedEntry,
}

/// 队列种类(对齐 TS 字面量)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueueKind {
    Steer,
    #[serde(rename = "followUp")]
    FollowUp,
    #[serde(rename = "nextRun")]
    NextRun,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueCancelledRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub entry_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteDeferredRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub run_id: String,
    pub target: ProvisionedEntry,
}

/// usage 记录的 cause 联合:字段扁平存放,有效性由 `cause` 判别
/// (assistant/compaction/branch_summary/deferred_fetch 需要 attempt+stopReason,
/// tool 需要 toolCallId,hook 仅 runId+entryId,adjustment 全可选+details)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub id: String,
    pub seq: i64,
    pub lane: String,
    pub timestamp: i64,
    pub usage: Usage,
    /// `"assistant" | "compaction" | "branch_summary" | "deferred_fetch" | "tool" | "hook" | "adjustment"`。
    pub cause: UsageCauseKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<SessionStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonValue>,
}

/// usage cause 判别(对齐 TS 联合的字面量)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCauseKind {
    Assistant,
    Compaction,
    BranchSummary,
    DeferredFetch,
    Tool,
    Hook,
    Adjustment,
}

/// lane 级记录(对齐 TS `LaneRecord`;`type` 为内部 tag)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LaneRecord {
    #[serde(rename = "operation_started")]
    OperationStarted(OperationStartedRecord),
    #[serde(rename = "abort_requested")]
    AbortRequested(AbortRequestedRecord),
    #[serde(rename = "operation_finished")]
    OperationFinished(OperationFinishedRecord),
    #[serde(rename = "step_attempt")]
    StepAttempt(StepAttemptRecord),
    #[serde(rename = "tool_started")]
    ToolStarted(ToolStartedRecord),
    #[serde(rename = "queue_enqueued")]
    QueueEnqueued(QueueEnqueuedRecord),
    #[serde(rename = "queue_cancelled")]
    QueueCancelled(QueueCancelledRecord),
    #[serde(rename = "write_deferred")]
    WriteDeferred(WriteDeferredRecord),
    #[serde(rename = "usage")]
    Usage(UsageRecord),
}

impl LaneRecord {
    pub fn id(&self) -> &str {
        match self {
            LaneRecord::OperationStarted(r) => &r.id,
            LaneRecord::AbortRequested(r) => &r.id,
            LaneRecord::OperationFinished(r) => &r.id,
            LaneRecord::StepAttempt(r) => &r.id,
            LaneRecord::ToolStarted(r) => &r.id,
            LaneRecord::QueueEnqueued(r) => &r.id,
            LaneRecord::QueueCancelled(r) => &r.id,
            LaneRecord::WriteDeferred(r) => &r.id,
            LaneRecord::Usage(r) => &r.id,
        }
    }

    pub fn seq(&self) -> i64 {
        match self {
            LaneRecord::OperationStarted(r) => r.seq,
            LaneRecord::AbortRequested(r) => r.seq,
            LaneRecord::OperationFinished(r) => r.seq,
            LaneRecord::StepAttempt(r) => r.seq,
            LaneRecord::ToolStarted(r) => r.seq,
            LaneRecord::QueueEnqueued(r) => r.seq,
            LaneRecord::QueueCancelled(r) => r.seq,
            LaneRecord::WriteDeferred(r) => r.seq,
            LaneRecord::Usage(r) => r.seq,
        }
    }

    pub fn lane(&self) -> &str {
        match self {
            LaneRecord::OperationStarted(r) => &r.lane,
            LaneRecord::AbortRequested(r) => &r.lane,
            LaneRecord::OperationFinished(r) => &r.lane,
            LaneRecord::StepAttempt(r) => &r.lane,
            LaneRecord::ToolStarted(r) => &r.lane,
            LaneRecord::QueueEnqueued(r) => &r.lane,
            LaneRecord::QueueCancelled(r) => &r.lane,
            LaneRecord::WriteDeferred(r) => &r.lane,
            LaneRecord::Usage(r) => &r.lane,
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            LaneRecord::OperationStarted(r) => r.timestamp,
            LaneRecord::AbortRequested(r) => r.timestamp,
            LaneRecord::OperationFinished(r) => r.timestamp,
            LaneRecord::StepAttempt(r) => r.timestamp,
            LaneRecord::ToolStarted(r) => r.timestamp,
            LaneRecord::QueueEnqueued(r) => r.timestamp,
            LaneRecord::QueueCancelled(r) => r.timestamp,
            LaneRecord::WriteDeferred(r) => r.timestamp,
            LaneRecord::Usage(r) => r.timestamp,
        }
    }

    pub fn record_type(&self) -> &'static str {
        match self {
            LaneRecord::OperationStarted(_) => "operation_started",
            LaneRecord::AbortRequested(_) => "abort_requested",
            LaneRecord::OperationFinished(_) => "operation_finished",
            LaneRecord::StepAttempt(_) => "step_attempt",
            LaneRecord::ToolStarted(_) => "tool_started",
            LaneRecord::QueueEnqueued(_) => "queue_enqueued",
            LaneRecord::QueueCancelled(_) => "queue_cancelled",
            LaneRecord::WriteDeferred(_) => "write_deferred",
            LaneRecord::Usage(_) => "usage",
        }
    }

    /// operation 归属 id:operation_started 用自身 id,其余取 runId。
    pub fn operation_identity(&self) -> Option<&str> {
        match self {
            LaneRecord::OperationStarted(record) => Some(&record.id),
            LaneRecord::AbortRequested(record) => Some(&record.run_id),
            LaneRecord::OperationFinished(record) => Some(&record.run_id),
            LaneRecord::StepAttempt(record) => Some(&record.run_id),
            LaneRecord::ToolStarted(record) => Some(&record.run_id),
            LaneRecord::QueueEnqueued(record) => record.run_id.as_deref(),
            LaneRecord::QueueCancelled(record) => record.run_id.as_deref(),
            LaneRecord::WriteDeferred(record) => Some(&record.run_id),
            LaneRecord::Usage(record) => record.run_id.as_deref(),
        }
    }
}

// ---------------------------------------------------------------------------
// 查询与视图类型
// ---------------------------------------------------------------------------

/// 条目/记录排序(对齐 TS `EntryOrder`)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryOrder {
    #[default]
    NewestFirst,
    OldestFirst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryCursor {
    pub after_seq: i64,
}

/// 条目查询(对齐 TS `EntryQuery`)。
#[derive(Clone, Debug, Default)]
pub struct EntryQuery {
    pub entry_type: Option<String>,
    /// 仅 type = "custom" 有效。
    pub custom_type: Option<String>,
    pub order: Option<EntryOrder>,
    pub limit: Option<usize>,
    pub cursor: Option<EntryCursor>,
}

/// 分支扫描边界(对齐 TS `BranchBounds`;默认整条路径、叶到根)。
#[derive(Clone, Debug, Default)]
pub struct BranchBounds {
    /// 缺省为视图 lane 的叶。
    pub start: Option<String>,
    /// 首个匹配后停止(含)。
    pub stop_at_type: Option<String>,
    pub stop_at_id: Option<String>,
}

/// 存储层分支查询(强制 start;对齐 TS `EntryQuery & BranchBounds & { start }`)。
#[derive(Clone, Debug, Default)]
pub struct BranchEntryQuery {
    pub query: EntryQuery,
    pub bounds: BranchBounds,
    pub start: String,
}

/// 记录查询(对齐 TS `RecordQuery`)。
#[derive(Clone, Debug, Default)]
pub struct RecordQuery {
    pub lane: Option<String>,
    pub record_type: Option<String>,
    pub run_id: Option<String>,
    /// 仅 type = "operation_started" 有效。
    pub operation_kind: Option<String>,
    /// 排他时序下界:seq > after_seq(与 order 无关)。
    pub after_seq: Option<i64>,
    pub order: Option<EntryOrder>,
    pub limit: Option<usize>,
}

/// 会话元数据(对齐 TS `SessionMetadata`)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

/// 会话统计(对齐 TS `SessionStats`)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub message_count: i64,
    pub cached_tokens: i64,
    pub uncached_tokens: i64,
    pub total_tokens: i64,
    pub cost_total: f64,
}

/// lane 指针(对齐 TS `LanePointer`)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanePointer {
    pub lane: String,
    pub leaf_id: Option<String>,
}

/// 日志项(对齐 TS `LogItem`;fact 内嵌子判别)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum LogItem {
    #[serde(rename = "entry")]
    Entry { seq: i64, entry: Entry },
    #[serde(rename = "record")]
    Record { seq: i64, record: LaneRecord },
    #[serde(rename = "lane")]
    Lane {
        seq: i64,
        lane: String,
        leaf_id: Option<String>,
    },
    #[serde(rename = "fact")]
    Fact { seq: i64, fact: SessionFact },
}

/// 全局事实载荷(对齐 TS fact 联合)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all_fields = "camelCase")]
pub enum SessionFact {
    #[serde(rename = "name")]
    Name {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename = "label")]
    Label {
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

impl LogItem {
    pub fn seq(&self) -> i64 {
        match self {
            LogItem::Entry { seq, .. }
            | LogItem::Record { seq, .. }
            | LogItem::Lane { seq, .. }
            | LogItem::Fact { seq, .. } => *seq,
        }
    }
}

/// 日志读取选项(对齐 TS `LogOptions`)。
#[derive(Clone, Copy, Debug, Default)]
pub struct LogOptions {
    pub after_seq: Option<i64>,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// SessionError
// ---------------------------------------------------------------------------

/// 会话层稳定错误码(对齐 TS `SessionErrorCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    NotFound,
    AlreadyExists,
    InvalidEntry,
    InvalidPayload,
    InvalidLane,
    InvalidQuery,
    InvalidForkTarget,
    Storage,
}

impl std::fmt::Display for SessionErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            SessionErrorCode::NotFound => "not_found",
            SessionErrorCode::AlreadyExists => "already_exists",
            SessionErrorCode::InvalidEntry => "invalid_entry",
            SessionErrorCode::InvalidPayload => "invalid_payload",
            SessionErrorCode::InvalidLane => "invalid_lane",
            SessionErrorCode::InvalidQuery => "invalid_query",
            SessionErrorCode::InvalidForkTarget => "invalid_fork_target",
            SessionErrorCode::Storage => "storage",
        };
        f.write_str(text)
    }
}

/// 会话层错误(对齐 TS `SessionError`)。
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
    #[source]
    pub cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl SessionError {
    pub fn new(code: SessionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
        }
    }

    pub fn with_cause(mut self, cause: Arc<dyn std::error::Error + Send + Sync>) -> Self {
        self.cause = Some(cause);
        self
    }
}

// ---------------------------------------------------------------------------
// SessionStorage / SessionTree trait
// ---------------------------------------------------------------------------

/// 会话存储能力(对齐 TS `SessionStorage`)。所有方法都可能失败/抛错 → `Result`。
pub trait SessionStorage: Send + Sync {
    fn get_metadata<'a>(&'a self) -> BoxFuture<'a, Result<SessionMetadata, SessionError>>;

    // Lanes
    fn get_lanes<'a>(&'a self) -> BoxFuture<'a, Result<Vec<LanePointer>, SessionError>>;
    fn create_lane<'a>(&'a self, lane: String, at: Option<String>)
        -> BoxFuture<'a, Result<(), SessionError>>;
    fn move_lane<'a>(&'a self, lane: String, to: Option<String>)
        -> BoxFuture<'a, Result<(), SessionError>>;

    // Entries and Records
    fn append_entry<'a>(
        &'a self,
        entry: ProvisionedEntry,
        lane: String,
    ) -> BoxFuture<'a, Result<Entry, SessionError>>;
    fn append_record<'a>(&'a self, record: LaneRecord)
        -> BoxFuture<'a, Result<LaneRecord, SessionError>>;

    // Reads
    fn get_entry<'a>(&'a self, id: String) -> BoxFuture<'a, Option<Entry>>;
    fn find_entries<'a>(&'a self, query: EntryQuery)
        -> BoxFuture<'a, Result<Vec<Entry>, SessionError>>;
    /// start 在存储层必填(视图层的 lane 叶缺省属于 SessionTree 糖)。
    fn find_entries_on_branch<'a>(
        &'a self,
        query: BranchEntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>>;
    fn find_records<'a>(
        &'a self,
        query: RecordQuery,
    ) -> BoxFuture<'a, Result<Vec<LaneRecord>, SessionError>>;
    /// 未完成的 operation_started,最新优先(恢复用 limit: 2)。
    fn find_open_operations<'a>(
        &'a self,
        lane: String,
        limit: Option<usize>,
    ) -> BoxFuture<'a, Result<Vec<OperationStartedRecord>, SessionError>>;
    fn get_log<'a>(&'a self, options: LogOptions)
        -> BoxFuture<'a, Result<Vec<LogItem>, SessionError>>;

    // Global facts
    fn get_name<'a>(&'a self) -> BoxFuture<'a, Option<String>>;
    fn set_name<'a>(&'a self, name: Option<String>) -> BoxFuture<'a, Result<(), SessionError>>;
    fn get_label<'a>(&'a self, id: String) -> BoxFuture<'a, Option<String>>;
    fn set_label<'a>(
        &'a self,
        id: String,
        label: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>>;
    fn get_stats<'a>(&'a self) -> BoxFuture<'a, Result<SessionStats, SessionError>>;
}

/// 会话树视图能力(对齐 TS `SessionTree`)。
pub trait SessionTree: Send + Sync {
    fn get_leaf_id<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, SessionError>>;
    fn get_entry<'a>(&'a self, id: String) -> BoxFuture<'a, Option<Entry>>;
    fn get_stats<'a>(&'a self) -> BoxFuture<'a, Result<SessionStats, SessionError>>;

    // Global facts;latest wins,not branch-scoped。
    fn get_name<'a>(&'a self) -> BoxFuture<'a, Option<String>>;
    fn set_name<'a>(&'a self, name: Option<String>) -> BoxFuture<'a, Result<(), SessionError>>;
    fn get_label<'a>(&'a self, target_id: String) -> BoxFuture<'a, Option<String>>;
    fn set_label<'a>(
        &'a self,
        target_id: String,
        label: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>>;

    /// 会话级、全分支、序列序。
    fn find_entries<'a>(&'a self, query: EntryQuery)
        -> BoxFuture<'a, Result<Vec<Entry>, SessionError>>;
    fn find_entry<'a>(&'a self, query: EntryQuery)
        -> BoxFuture<'a, Result<Option<Entry>, SessionError>>;

    /// 分支域:从 start 向根的路径。
    fn find_entries_on_branch<'a>(
        &'a self,
        query: BranchQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>>;
    fn find_entry_on_branch<'a>(
        &'a self,
        query: BranchQuery,
    ) -> BoxFuture<'a, Result<Option<Entry>, SessionError>>;

    // Writes;返回条目 id(写入延迟时为 provisioned id)。
    fn append_message<'a>(&'a self, message: AgentMessage)
        -> BoxFuture<'a, Result<String, SessionError>>;
    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<Value>,
    ) -> BoxFuture<'a, Result<String, SessionError>>;
}

/// `SessionTree.find_entries_on_branch` 的查询(start 可缺省 = 视图 lane 叶)。
#[derive(Clone, Debug, Default)]
pub struct BranchQuery {
    pub query: EntryQuery,
    pub bounds: BranchBounds,
}

/// 会话创建选项(对齐 TS `SessionCreateOptions`)。
#[derive(Clone, Debug, Default)]
pub struct SessionCreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
}

/// fork 范围与位置(对齐 TS `ForkOptions`)。
#[derive(Clone, Debug, Default)]
pub struct ForkOptions {
    /// `"branch"`(缺省)或 `"tree"`。
    pub scope: Option<ForkScope>,
    pub entry_id: Option<String>,
    /// `"before" | "at"`;缺省:entryId 缺省时 "at",否则 "before"。
    pub position: Option<ForkPosition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkScope {
    Branch,
    Tree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkPosition {
    Before,
    At,
}
