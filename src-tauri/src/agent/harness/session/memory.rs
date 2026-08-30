//! 内存会话存储与仓库:对齐 `packages/agent/src/harness/session/memory.ts`。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::state::SessionState;
#[allow(unused_imports)]
use super::types::{
    ActiveToolsEntry, BranchEntryQuery, BranchSummaryEntry, CompactionEntry, CustomEntry, Entry,
    EntryQuery, ForkOptions, LanePointer, LaneRecord, LogItem, LogOptions, MessageEntry,
    ModelChangeEntry, OperationStartedRecord, ProvisionedEntry, ProvisionedActiveToolsEntry,
    ProvisionedBranchSummaryEntry, ProvisionedCompactionEntry, ProvisionedCustomEntry,
    ProvisionedModelChangeEntry, ProvisionedThinkingLevelEntry, RecordQuery, SessionCreateOptions,
    SessionError, SessionErrorCode, SessionFact, SessionMetadata, SessionStats, SessionStorage,
    ThinkingLevelEntry,
};
use crate::agent::agent_loop::now_ms;

/// TS `structuredClone` 对应:值类型均可 Clone,直接克隆。
fn cloned<T: Clone>(value: T) -> T {
    value
}

/// 内存 [`SessionStorage`] 实现。
pub struct InMemorySessionStorage {
    metadata: SessionMetadata,
    state: Mutex<SessionState>,
}

impl InMemorySessionStorage {
    pub fn new(metadata: SessionMetadata) -> Self {
        Self {
            metadata: cloned(metadata),
            state: Mutex::new(SessionState::new()),
        }
    }

    /// fork:把源存储的 fork 变更序列应用到新存储。
    pub fn fork(&self, metadata: SessionMetadata, options: &ForkOptions) -> Result<Self, SessionError> {
        let storage = Self::new(metadata);
        {
            let source = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let mutations = source.create_fork_mutations(options)?;
            let mut target = storage.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            for mutation in mutations {
                target.apply_mutation(&mutation)?;
            }
        }
        Ok(storage)
    }

    /// 直接访问内部状态(测试与 fork 用)。
    #[doc(hidden)]
    pub fn state_lock(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl SessionStorage for InMemorySessionStorage {
    fn get_metadata<'a>(&'a self) -> BoxFuture<'a, Result<SessionMetadata, SessionError>> {
        Box::pin(async move { Ok(cloned(self.metadata.clone())) })
    }

    fn get_lanes<'a>(&'a self) -> BoxFuture<'a, Result<Vec<LanePointer>, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(state.get_lanes())
        })
    }

    fn create_lane<'a>(
        &'a self,
        lane: String,
        at: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            let mut state = self.state_lock();
            state.validate_new_lane(&lane)?;
            state.validate_target(at.as_deref())?;
            let seq = state.next_sequence();
            state.apply_mutation(&super::state::SessionMutation::Lane {
                seq,
                lane,
                leaf_id: at,
            })
        })
    }

    fn move_lane<'a>(
        &'a self,
        lane: String,
        to: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            let mut state = self.state_lock();
            state.require_lane(&lane)?;
            state.validate_target(to.as_deref())?;
            let seq = state.next_sequence();
            state.apply_mutation(&super::state::SessionMutation::Lane {
                seq,
                lane,
                leaf_id: to,
            })
        })
    }

    fn append_entry<'a>(
        &'a self,
        new_entry: ProvisionedEntry,
        lane: String,
    ) -> BoxFuture<'a, Result<Entry, SessionError>> {
        Box::pin(async move {
            let mut state = self.state_lock();
            let parent_id = state.require_lane(&lane)?;
            state.validate_unused_id(new_entry.id())?;
            let entry = provisioned_to_entry(new_entry, parent_id, state.next_sequence(), now_ms());
            state.apply_mutation(&super::state::SessionMutation::Entry {
                lane: Some(lane),
                entry: entry.clone(),
            })?;
            Ok(cloned(entry))
        })
    }

    fn append_record<'a>(
        &'a self,
        new_record: LaneRecord,
    ) -> BoxFuture<'a, Result<LaneRecord, SessionError>> {
        Box::pin(async move {
            let mut state = self.state_lock();
            state.require_lane(new_record.lane())?;
            state.validate_unused_id(new_record.id())?;
            let current_open_operation_id = state
                .find_open_operations(new_record.lane(), Some(1))?
                .first()
                .map(|record| record.id.clone());
            if let (LaneRecord::OperationStarted(_), Some(current_open_operation_id)) =
                (&new_record, current_open_operation_id)
            {
                return Err(SessionError::new(
                    SessionErrorCode::Storage,
                    format!(
                        "Lane {} already has an open operation {}",
                        new_record.lane(),
                        current_open_operation_id
                    ),
                ));
            }
            let record = with_seq_timestamp(new_record, state.next_sequence(), now_ms());
            state.apply_mutation(&super::state::SessionMutation::Record {
                record: record.clone(),
            })?;
            Ok(cloned(record))
        })
    }

    fn get_entry<'a>(&'a self, id: String) -> BoxFuture<'a, Option<Entry>> {
        Box::pin(async move {
            let state = self.state_lock();
            state.get_entry(&id).cloned()
        })
    }

    fn find_entries<'a>(
        &'a self,
        query: EntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(cloned(state.find_entries(&query)?))
        })
    }

    fn find_entries_on_branch<'a>(
        &'a self,
        query: BranchEntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(cloned(state.find_entries_on_branch(&query.query, &query.bounds, &query.start)?))
        })
    }

    fn find_records<'a>(
        &'a self,
        query: RecordQuery,
    ) -> BoxFuture<'a, Result<Vec<LaneRecord>, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(cloned(state.find_records(&query)?))
        })
    }

    fn find_open_operations<'a>(
        &'a self,
        lane: String,
        limit: Option<usize>,
    ) -> BoxFuture<'a, Result<Vec<OperationStartedRecord>, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(cloned(state.find_open_operations(&lane, limit)?))
        })
    }

    fn get_log<'a>(&'a self, options: LogOptions) -> BoxFuture<'a, Result<Vec<LogItem>, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(cloned(state.get_log(&options)?))
        })
    }

    fn get_name<'a>(&'a self) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let state = self.state_lock();
            state.get_name()
        })
    }

    fn set_name<'a>(&'a self, name: Option<String>) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            let mut state = self.state_lock();
            let seq = state.next_sequence();
            state.apply_mutation(&super::state::SessionMutation::Fact {
                seq,
                fact: super::types::SessionFact::Name { name },
            })
        })
    }

    fn get_label<'a>(&'a self, id: String) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let state = self.state_lock();
            state.get_label(&id)
        })
    }

    fn set_label<'a>(
        &'a self,
        id: String,
        label: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            let mut state = self.state_lock();
            state.validate_target(Some(&id))?;
            let seq = state.next_sequence();
            state.apply_mutation(&super::state::SessionMutation::Fact {
                seq,
                fact: super::types::SessionFact::Label {
                    target_id: id,
                    label,
                },
            })
        })
    }

    fn get_stats<'a>(&'a self) -> BoxFuture<'a, Result<SessionStats, SessionError>> {
        Box::pin(async move {
            let state = self.state_lock();
            Ok(cloned(state.get_stats()))
        })
    }
}

/// `ProvisionedEntry` → 存储层 `Entry`(补 parentId/seq/timestamp)。
fn provisioned_to_entry(
    provisioned: ProvisionedEntry,
    parent_id: Option<String>,
    seq: i64,
    timestamp: i64,
) -> Entry {
    match provisioned {
        ProvisionedEntry::Message(e) => Entry::Message(MessageEntry {
            id: e.id,
            seq,
            parent_id,
            timestamp,
            message: e.message,
            terminate: e.terminate,
        }),
        ProvisionedEntry::ModelChange(e) => Entry::ModelChange(ModelChangeEntry {
            id: e.id,
            seq,
            parent_id,
            timestamp,
            provider: e.provider,
            model_id: e.model_id,
        }),
        ProvisionedEntry::ThinkingLevelChange(e) => {
            Entry::ThinkingLevelChange(ThinkingLevelEntry {
                id: e.id,
                seq,
                parent_id,
                timestamp,
                thinking_level: e.thinking_level,
            })
        }
        ProvisionedEntry::ActiveToolsChange(e) => Entry::ActiveToolsChange(ActiveToolsEntry {
            id: e.id,
            seq,
            parent_id,
            timestamp,
            active_tool_names: e.active_tool_names,
        }),
        ProvisionedEntry::Compaction(e) => Entry::Compaction(CompactionEntry {
            id: e.id,
            seq,
            parent_id,
            timestamp,
            summary: e.summary,
            retained_tail: e.retained_tail,
            tokens_before: e.tokens_before,
            details: e.details,
            usage: e.usage,
        }),
        ProvisionedEntry::BranchSummary(e) => Entry::BranchSummary(BranchSummaryEntry {
            id: e.id,
            seq,
            parent_id,
            timestamp,
            from_id: e.from_id,
            summary: e.summary,
            details: e.details,
            usage: e.usage,
        }),
        ProvisionedEntry::Custom(e) => Entry::Custom(CustomEntry {
            id: e.id,
            seq,
            parent_id,
            timestamp,
            custom_type: e.custom_type,
            data: e.data,
        }),
    }
}

/// 新记录补 seq/timestamp(构造时占位值由存储覆盖)。
fn with_seq_timestamp(mut record: LaneRecord, seq: i64, timestamp: i64) -> LaneRecord {
    match &mut record {
        LaneRecord::OperationStarted(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::AbortRequested(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::OperationFinished(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::StepAttempt(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::ToolStarted(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::QueueEnqueued(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::QueueCancelled(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::WriteDeferred(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
        LaneRecord::Usage(r) => {
            r.seq = seq;
            r.timestamp = timestamp;
        }
    }
    record
}

/// JSONL 存储层复用入口(与内存存储共享 provisioned→entry 装配)。
pub(crate) fn provisioned_to_entry_for_storage(
    provisioned: ProvisionedEntry,
    parent_id: Option<String>,
    seq: i64,
    timestamp: i64,
) -> Entry {
    provisioned_to_entry(provisioned, parent_id, seq, timestamp)
}

/// JSONL 存储层复用入口(与内存存储共享 seq/timestamp 装配)。
pub(crate) fn with_seq_timestamp_for_storage(
    record: LaneRecord,
    seq: i64,
    timestamp: i64,
) -> LaneRecord {
    with_seq_timestamp(record, seq, timestamp)
}

/// 内存会话仓库(对齐 TS `InMemorySessionRepo`)。
#[derive(Default)]
pub struct InMemorySessionRepo {
    sessions: Mutex<HashMap<String, Arc<InMemorySessionStorage>>>,
}

impl InMemorySessionRepo {
    pub fn new() -> Self {
        Self::default()
    }

    fn require_storage(
        &self,
        id: &str,
    ) -> Result<Arc<InMemorySessionStorage>, SessionError> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| {
                SessionError::new(
                    SessionErrorCode::NotFound,
                    format!("Session not found: {id}"),
                )
            })
    }

    /// 创建会话(对齐 TS `create`;id 缺省 UUIDv7)。
    pub async fn create(
        &self,
        options: SessionCreateOptions,
    ) -> Result<super::session::Session, SessionError> {
        let id = options
            .id
            .clone()
            .unwrap_or_else(crate::agent::harness::uuid::uuid_v7);
        {
            let sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if sessions.contains_key(&id) {
                return Err(SessionError::new(
                    SessionErrorCode::AlreadyExists,
                    format!("Session already exists: {id}"),
                ));
            }
        }
        let storage = Arc::new(InMemorySessionStorage::new(SessionMetadata {
            id,
            created_at: now_ms(),
            parent_session_id: options.parent_session_id,
        }));
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(storage.metadata.id.clone(), storage.clone());
        Ok(super::session::Session::new(storage))
    }

    /// 打开会话(对齐 TS `open`;内存实现无写者声明)。
    pub async fn open(
        &self,
        metadata: &SessionMetadata,
    ) -> Result<super::session::Session, SessionError> {
        Ok(super::session::Session::new(self.require_storage(&metadata.id)?))
    }

    /// 列出会话元数据(对齐 TS `list`)。
    pub async fn list(&self) -> Vec<SessionMetadata> {
        let sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions
            .values()
            .map(|storage| storage.metadata.clone())
            .collect()
    }

    /// 删除会话(对齐 TS `delete`)。
    pub async fn delete(&self, metadata: &SessionMetadata) {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&metadata.id);
    }

    /// fork 会话(对齐 TS `fork`;parentSessionId 缺省取源会话 id)。
    pub async fn fork(
        &self,
        source: &SessionMetadata,
        options: ForkOptions,
        create: SessionCreateOptions,
    ) -> Result<super::session::Session, SessionError> {
        let source_storage = self.require_storage(&source.id)?;
        let id = create
            .id
            .clone()
            .unwrap_or_else(crate::agent::harness::uuid::uuid_v7);
        {
            let sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if sessions.contains_key(&id) {
                return Err(SessionError::new(
                    SessionErrorCode::AlreadyExists,
                    format!("Session already exists: {id}"),
                ));
            }
        }
        let storage = Arc::new(source_storage.fork(
            SessionMetadata {
                id,
                created_at: now_ms(),
                parent_session_id: create.parent_session_id.or_else(|| Some(source.id.clone())),
            },
            &options,
        )?);
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(storage.metadata.id.clone(), storage.clone());
        Ok(super::session::Session::new(storage))
    }
}
