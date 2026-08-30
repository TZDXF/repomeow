//! session 子系统:对齐 `packages/agent/src/harness/session/index.ts` + `jsonl.ts`。

pub mod context;
pub mod jsonl;
pub mod memory;
pub mod session;
pub mod state;
pub mod testing;
pub mod types;

#[allow(unused_imports)]
pub use context::{
    build_context_entries, build_session_context, default_context_entry_transform,
    session_entry_to_context_messages, ContextEntryTransform, CustomEntryContextMessageProjector,
    SessionContext, SessionContextBuildOptions, SessionModelRef,
};
#[allow(unused_imports)]
pub use jsonl::{
    JsonlSessionCreateOptions, JsonlSessionListOptions, JsonlSessionMetadata, JsonlSessionRepo,
    JsonlSessionRepoOptions, JsonlSessionStorage, JsonlV4Header,
};
#[allow(unused_imports)]
pub use memory::{InMemorySessionRepo, InMemorySessionStorage};
#[allow(unused_imports)]
pub use session::{LaneView, Session};
#[allow(unused_imports)]
pub use state::SessionMutation;
#[allow(unused_imports)]
pub use types::{
    BranchBounds, BranchEntryQuery, BranchQuery, CompactionReason, Entry, EntryCursor, EntryOrder,
    EntryQuery, ForkOptions, ForkPosition, ForkScope, IdGenerator, JsonValue, LanePointer,
    LaneRecord, LogItem, LogOptions, OperationIntent, OperationOutcome, ProvisionedEntry,
    QueueKind, RecordQuery, SessionCreateOptions, SessionError, SessionErrorCode, SessionFact,
    SessionMetadata, SessionStats, SessionStopReason, SessionStorage, SessionTree, StepKind,
    ToolReplay, UuidIdGenerator, UsageCauseKind, UsageRecord,
};
