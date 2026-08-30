//! JSONL 会话存储:对齐 `packages/agent/src/harness/session/jsonl/storage.ts`。
//!
//! 写入经 `tokio::sync::Mutex` 串行化(对齐 TS 的 promise tail 链);读取与查询
//! 直接作用于内存 `SessionState`。torn tail(最后一行 JSON 截断)在 load 时
//! 通过「临时文件 + 原子重命名」发布合法前缀修复。

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::Mutex;

use super::codec::{encode_header, encode_mutation, metadata_from_header, parse_header, parse_mutation};
use super::errors::{file_result, invalid_file, JsonlDecodeError, JsonlDecodeErrorKind};
use super::types::{JsonlSessionMetadata, JsonlV4Header};
use crate::agent::agent_loop::now_ms;
use crate::agent::harness::session::state::SessionState;
use crate::agent::harness::session::types::{
    BranchEntryQuery, Entry, EntryQuery, LanePointer, LaneRecord, LogItem, LogOptions,
    OperationStartedRecord, ProvisionedEntry, RecordQuery, SessionError, SessionErrorCode,
    SessionFact, SessionMetadata, SessionStats, SessionStorage,
};
use crate::agent::harness::types::{
    CreateDirOptions, CreateTempFileOptions, FileContent, FileSystem, ReadTextLinesOptions,
    RemoveOptions, Result,
};

/// 「临时文件 + 原子重命名」发布完整文件(对齐 TS `publishFileAtomically`)。
async fn publish_file_atomically<P, Fut>(
    fs: &dyn FileSystem,
    destination_path: &str,
    populate: P,
) -> Result<(), SessionError>
where
    P: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), SessionError>>,
{
    let temp_path = format!("{destination_path}.tmp");
    let result = async {
        populate(temp_path.clone()).await?;
        file_result(
            fs.rename_file(temp_path.clone(), destination_path.to_string(), None)
                .await,
            &format!("Failed to publish staged file {destination_path}"),
        )
    }
    .await;
    if result.is_err() {
        // 临时文件清理尽力而为,保留原错误。
        let _ = fs
            .remove(
                temp_path,
                RemoveOptions {
                    recursive: None,
                    force: Some(true),
                    abort_signal: None,
                },
            )
            .await;
    }
    result
}

/// JSONL 会话存储(对齐 TS `JsonlSessionStorage`)。
pub struct JsonlSessionStorage {
    fs: Arc<dyn FileSystem>,
    metadata: JsonlSessionMetadata,
    state: std::sync::Mutex<SessionState>,
    /// 写入串行化锁(对齐 TS tail promise 链)。
    tail: Arc<Mutex<()>>,
}

impl JsonlSessionStorage {
    pub fn new(fs: Arc<dyn FileSystem>, metadata: JsonlSessionMetadata) -> Self {
        Self {
            fs,
            metadata,
            state: std::sync::Mutex::new(SessionState::new()),
            tail: Arc::new(Mutex::new(())),
        }
    }

    /// 创建新会话文件并返回存储(对齐 TS `create`)。
    pub async fn create(
        fs: Arc<dyn FileSystem>,
        path: &str,
        header: JsonlV4Header,
    ) -> Result<Self, SessionError> {
        file_result(
            fs.write_file(
                path.to_string(),
                FileContent::Text(encode_header(&header)),
                None,
            )
            .await,
            &format!("Failed to initialize session {path}"),
        )?;
        let file_info = file_result(
            fs.file_info(path.to_string()).await,
            &format!("Failed to read session metadata {path}"),
        )?;
        Ok(Self::new(
            fs,
            metadata_from_header(&header, path, file_info.mtime_ms),
        ))
    }

    /// 加载既有会话文件(对齐 TS `load`),含 torn-tail 修复。
    pub async fn load(fs: Arc<dyn FileSystem>, path: &str) -> Result<Self, SessionError> {
        let content = file_result(
            fs.read_text_file(path.to_string(), None).await,
            &format!("Failed to read session {path}"),
        )?;
        let mut physical_lines: Vec<&str> = content.split('\n').collect();
        if physical_lines.last() == Some(&"") {
            physical_lines.pop();
        }
        if physical_lines.is_empty() || physical_lines[0].is_empty() {
            return Err(invalid_file(
                path,
                1,
                &JsonlDecodeError::new(JsonlDecodeErrorKind::Schema, "is missing a header"),
            ));
        }
        let header = parse_header(physical_lines[0])
            .map_err(|error| invalid_file(path, 1, &error))?;
        let file_info = file_result(
            fs.file_info(path.to_string()).await,
            &format!("Failed to read session metadata {path}"),
        )?;
        let storage = Self::new(fs.clone(), metadata_from_header(&header, path, file_info.mtime_ms));
        let mut torn_tail_repaired = false;
        for (offset, line) in physical_lines.iter().skip(1).enumerate() {
            let index = offset + 1; // 1-based 行号(蓝本 index 从 1 开始)
            let mutation = match parse_mutation(line) {
                Ok(mutation) => mutation,
                Err(error) => {
                    let is_torn_tail = index == physical_lines.len() - 1
                        && error.kind == JsonlDecodeErrorKind::Syntax;
                    if is_torn_tail {
                        let valid_prefix = format!("{}\n", physical_lines[..index].join("\n"));
                        publish_file_atomically(fs.as_ref(), path, {
                            let fs = fs.clone();
                            let valid_prefix = valid_prefix.clone();
                            move |temp_path: String| async move {
                                file_result(
                                    fs.write_file(temp_path, FileContent::Text(valid_prefix), None)
                                        .await,
                                    &format!("Failed to stage torn-tail repair {path}"),
                                )
                            }
                        })
                        .await?;
                        torn_tail_repaired = true;
                        break;
                    }
                    return Err(invalid_file(path, index + 1, &error));
                }
            };
            let mut state = storage.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(error) = state.apply_mutation(&mutation) {
                if error.code == SessionErrorCode::InvalidEntry {
                    return Err(invalid_file(path, index + 1, &error));
                }
                return Err(error);
            }
        }
        if !torn_tail_repaired && !content.ends_with('\n') {
            file_result(
                fs.append_file(path.to_string(), FileContent::Text("\n".to_string()))
                    .await,
                &format!("Failed to repair unterminated session tail {path}"),
            )?;
        }
        Ok(storage)
    }

    /// fork:把 fork 变更序列写入新文件(对齐 TS `fork`)。
    pub async fn fork(
        &self,
        path: &str,
        header: JsonlV4Header,
        options: &super::super::types::ForkOptions,
    ) -> Result<Self, SessionError> {
        let mutations = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .create_fork_mutations(options)?;
        publish_file_atomically(self.fs.as_ref(), path, {
            let fs = self.fs.clone();
            let header = header.clone();
            let mutations = mutations.clone();
            move |temp_path: String| async move {
                let target = Self::create(fs.clone(), &temp_path, header).await?;
                for mutation in &mutations {
                    target.append_mutation(mutation).await?;
                    target
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .apply_mutation(mutation)?;
                }
                Ok(())
            }
        })
        .await?;
        Self::load(self.fs.clone(), path).await
    }

    /// 等待挂起的写操作完成(对齐 TS `drain`)。
    pub async fn drain(&self) {
        let _guard = self.tail.lock().await;
    }

    pub async fn metadata(&self) -> JsonlSessionMetadata {
        self.metadata.clone()
    }

    async fn enqueue<T, P, Fut>(&self, operation: P) -> Result<T, SessionError>
    where
        P: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, SessionError>>,
    {
        let _guard = self.tail.lock().await;
        operation().await
    }

    async fn append_mutation(&self, mutation: &SessionMutationFor) -> Result<(), SessionError> {
        file_result(
            self.fs
                .append_file(
                    self.metadata.path.clone(),
                    FileContent::Text(encode_mutation(mutation)),
                )
                .await,
            &format!("Failed to append session {}", self.metadata.path),
        )
    }

    fn state_lock(&self) -> std::sync::MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

type SessionMutationFor = crate::agent::harness::session::state::SessionMutation;

impl SessionStorage for JsonlSessionStorage {
    fn get_metadata<'a>(&'a self) -> BoxFuture<'a, Result<SessionMetadata, SessionError>> {
        Box::pin(async move {
            Ok(SessionMetadata {
                id: self.metadata.id.clone(),
                created_at: self.metadata.created_at,
                parent_session_id: self.metadata.parent_session_id.clone(),
            })
        })
    }

    fn get_lanes<'a>(&'a self) -> BoxFuture<'a, Result<Vec<LanePointer>, SessionError>> {
        Box::pin(async move { Ok(self.state_lock().get_lanes()) })
    }

    fn create_lane<'a>(
        &'a self,
        lane: String,
        at: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            self.enqueue(|| {
                Box::pin(async move {
                    let mutation = {
                        let state = self.state_lock();
                        state.validate_new_lane(&lane)?;
                        state.validate_target(at.as_deref())?;
                        crate::agent::harness::session::state::SessionMutation::Lane {
                            seq: state.next_sequence(),
                            lane: lane.clone(),
                            leaf_id: at.clone(),
                        }
                    };
                    self.append_mutation(&mutation).await?;
                    self.state_lock().apply_mutation(&mutation)?;
                    Ok(())
                })
            })
            .await
        })
    }

    fn move_lane<'a>(
        &'a self,
        lane: String,
        to: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            self.enqueue(|| {
                Box::pin(async move {
                    let mutation = {
                        let state = self.state_lock();
                        state.require_lane(&lane)?;
                        state.validate_target(to.as_deref())?;
                        crate::agent::harness::session::state::SessionMutation::Lane {
                            seq: state.next_sequence(),
                            lane: lane.clone(),
                            leaf_id: to.clone(),
                        }
                    };
                    self.append_mutation(&mutation).await?;
                    self.state_lock().apply_mutation(&mutation)?;
                    Ok(())
                })
            })
            .await
        })
    }

    fn append_entry<'a>(
        &'a self,
        new_entry: ProvisionedEntry,
        lane: String,
    ) -> BoxFuture<'a, Result<Entry, SessionError>> {
        Box::pin(async move {
            self.enqueue(|| {
                Box::pin(async move {
                    let entry = {
                        let state = self.state_lock();
                        let parent_id = state.require_lane(&lane)?;
                        state.validate_unused_id(new_entry.id())?;
                        super::super::memory::provisioned_to_entry_for_storage(
                            new_entry,
                            parent_id,
                            state.next_sequence(),
                            now_ms(),
                        )
                    };
                    let mutation = crate::agent::harness::session::state::SessionMutation::Entry {
                        lane: Some(lane),
                        entry: entry.clone(),
                    };
                    self.append_mutation(&mutation).await?;
                    self.state_lock().apply_mutation(&mutation)?;
                    Ok(entry)
                })
            })
            .await
        })
    }

    fn append_record<'a>(
        &'a self,
        new_record: LaneRecord,
    ) -> BoxFuture<'a, Result<LaneRecord, SessionError>> {
        Box::pin(async move {
            self.enqueue(|| {
                Box::pin(async move {
                    let record = {
                        let state = self.state_lock();
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
                        super::super::memory::with_seq_timestamp_for_storage(
                            new_record,
                            state.next_sequence(),
                            now_ms(),
                        )
                    };
                    let mutation = crate::agent::harness::session::state::SessionMutation::Record {
                        record: record.clone(),
                    };
                    self.append_mutation(&mutation).await?;
                    self.state_lock().apply_mutation(&mutation)?;
                    Ok(record)
                })
            })
            .await
        })
    }

    fn get_entry<'a>(&'a self, id: String) -> BoxFuture<'a, Option<Entry>> {
        Box::pin(async move { self.state_lock().get_entry(&id).cloned() })
    }

    fn find_entries<'a>(
        &'a self,
        query: EntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move { Ok(self.state_lock().find_entries(&query)?) })
    }

    fn find_entries_on_branch<'a>(
        &'a self,
        query: BranchEntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move {
            Ok(self
                .state_lock()
                .find_entries_on_branch(&query.query, &query.bounds, &query.start)?)
        })
    }

    fn find_records<'a>(
        &'a self,
        query: RecordQuery,
    ) -> BoxFuture<'a, Result<Vec<LaneRecord>, SessionError>> {
        Box::pin(async move { Ok(self.state_lock().find_records(&query)?) })
    }

    fn find_open_operations<'a>(
        &'a self,
        lane: String,
        limit: Option<usize>,
    ) -> BoxFuture<'a, Result<Vec<OperationStartedRecord>, SessionError>> {
        Box::pin(async move { Ok(self.state_lock().find_open_operations(&lane, limit)?) })
    }

    fn get_log<'a>(&'a self, options: LogOptions) -> BoxFuture<'a, Result<Vec<LogItem>, SessionError>> {
        Box::pin(async move { Ok(self.state_lock().get_log(&options)?) })
    }

    fn get_name<'a>(&'a self) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.state_lock().get_name() })
    }

    fn set_name<'a>(&'a self, name: Option<String>) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            self.enqueue(|| {
                Box::pin(async move {
                    let mutation = {
                        let state = self.state_lock();
                        crate::agent::harness::session::state::SessionMutation::Fact {
                            seq: state.next_sequence(),
                            fact: SessionFact::Name { name: name.clone() },
                        }
                    };
                    self.append_mutation(&mutation).await?;
                    self.state_lock().apply_mutation(&mutation)?;
                    Ok(())
                })
            })
            .await
        })
    }

    fn get_label<'a>(&'a self, id: String) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.state_lock().get_label(&id) })
    }

    fn set_label<'a>(
        &'a self,
        id: String,
        label: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            self.enqueue(|| {
                Box::pin(async move {
                    let mutation = {
                        let state = self.state_lock();
                        state.validate_target(Some(&id))?;
                        crate::agent::harness::session::state::SessionMutation::Fact {
                            seq: state.next_sequence(),
                            fact: SessionFact::Label {
                                target_id: id.clone(),
                                label: label.clone(),
                            },
                        }
                    };
                    self.append_mutation(&mutation).await?;
                    self.state_lock().apply_mutation(&mutation)?;
                    Ok(())
                })
            })
            .await
        })
    }

    fn get_stats<'a>(&'a self) -> BoxFuture<'a, Result<SessionStats, SessionError>> {
        Box::pin(async move { Ok(self.state_lock().get_stats()) })
    }
}

// createTempFileOptions 引用占位:存储层当前不直接创建临时文件。
#[allow(dead_code)]
type _CreateTempFileOptions = CreateTempFileOptions;
#[allow(dead_code)]
type _CreateDirOptions = CreateDirOptions;
#[allow(dead_code)]
type _ReadTextLinesOptions = ReadTextLinesOptions;
