//! harness 专用错误族:对齐 `agent-harness.ts` 顶部定义的 TaggedError 类与
//! `HarnessFault`/`HarnessClosed`/`HarnessNotImplemented`。
//!
//! 每个错误的载荷字段与 TS 构造参数一一对应;`message` 字段由调用方提供。

use crate::harness_tagged_error;

harness_tagged_error!(
    /// 目标 lane 已被某个 operation 占用(对齐 TS `LaneBusy`)。
    LaneBusy,
    "LaneBusy" {
        pub lane: String,
        pub operation_id: String,
        pub operation_kind: String,
    }
);

harness_tagged_error!(
    /// 恢复所需身份(工具/模型)在当前配置中缺失(对齐 TS `MissingIdentities`)。
    MissingIdentities,
    "MissingIdentities" {
        pub lane: String,
        pub tools: Vec<String>,
        pub models: Vec<String>,
    }
);

harness_tagged_error!(
    /// lane 当前没有活跃 run(对齐 TS `NoActiveRun`)。
    NoActiveRun,
    "NoActiveRun" {
        pub lane: String,
    }
);

harness_tagged_error!(
    /// lane 当前没有活跃 operation(对齐 TS `NoActiveOperation`)。
    NoActiveOperation,
    "NoActiveOperation" {
        pub lane: String,
    }
);

harness_tagged_error!(
    /// lane 没有可恢复的挂起 operation(对齐 TS `NothingToResume`)。
    NothingToResume,
    "NothingToResume" {
        pub lane: String,
    }
);

harness_tagged_error!(
    /// 输入消息不满足运行约束(对齐 TS `InvalidMessage`)。
    InvalidMessage,
    "InvalidMessage" {
        pub lane: String,
        pub reason: String,
    }
);

harness_tagged_error!(
    /// 技能名未注册(对齐 TS `UnknownSkill`)。
    UnknownSkill,
    "UnknownSkill" {
        pub name: String,
    }
);

harness_tagged_error!(
    /// 提示词模板名未注册(对齐 TS `UnknownTemplate`)。
    UnknownTemplate,
    "UnknownTemplate" {
        pub name: String,
    }
);

harness_tagged_error!(
    /// 会话树目标条目不存在(对齐 TS `UnknownTarget`)。
    UnknownTarget,
    "UnknownTarget" {
        pub target_id: String,
    }
);

harness_tagged_error!(
    /// 排队条目 id 不属于任何队列(对齐 TS `UnknownQueueItem`)。
    UnknownQueueItem,
    "UnknownQueueItem" {
        pub lane: String,
        pub entry_id: String,
    }
);

harness_tagged_error!(
    /// lane 已存在(对齐 TS `LaneExists`)。
    LaneExists,
    "LaneExists" {
        pub lane: String,
    }
);

harness_tagged_error!(
    /// lane 名不合法(对齐 TS `InvalidLane`)。
    InvalidLane,
    "InvalidLane" {
        pub lane: String,
        pub reason: String,
    }
);

harness_tagged_error!(
    /// 没有可压缩内容(对齐 TS `NothingToCompact`)。
    NothingToCompact,
    "NothingToCompact" {
        pub lane: String,
    }
);

harness_tagged_error!(
    /// harness 已关闭(对齐 TS `Closed`)。
    Closed,
    "Closed" {}
);

/// 非预期内部故障(对齐 TS `HarnessFault extends Error`,携带 cause)。
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct HarnessFault {
    pub message: String,
    #[source]
    pub cause: Option<std::sync::Arc<dyn std::error::Error + Send + Sync>>,
}

impl HarnessFault {
    pub fn new(
        message: impl Into<String>,
        cause: Option<std::sync::Arc<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            message: message.into(),
            cause,
        }
    }
}

/// harness 在 operation 进行中被关闭(对齐 TS `HarnessClosed`)。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("AgentHarness was closed while the operation was active")]
pub struct HarnessClosed;

/// harness 运行方法尚未实现(WIP 骨架;对齐 TS `HarnessNotImplemented`)。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("AgentHarness.{operation} is not implemented yet")]
pub struct HarnessNotImplemented {
    pub operation: String,
}

impl HarnessNotImplemented {
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
        }
    }
}

/// WIP 骨架运行方法的统一错误:未实现,或已关闭。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(transparent)]
pub enum HarnessUnavailable {
    #[error(transparent)]
    NotImplemented(#[from] HarnessNotImplemented),
    #[error(transparent)]
    Closed(#[from] HarnessClosed),
}

/// operation 结束记录里的错误负载(对齐 TS `OperationError`)。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationError {
    pub code: String,
    pub message: String,
}
