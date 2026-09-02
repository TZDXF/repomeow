//! 会话门面:对齐 `packages/agent/src/harness/session/session.ts`。
//!
//! `Session` 持有 [`SessionStorage`] 并实现 [`SessionTree`](main lane 视图);
//! `view(lane)` 返回绑定到指定 lane 的 [`LaneView`]。写入前做 JSON 可序列化校验
//! (`assertJsonSerializable`;Rust 的 serde_json 值域天然不含环/非有限数/符号键,
//! 校验保留为防御性递归检查)。

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use super::types::{
    BranchQuery, Entry, EntryQuery, IdGenerator, LanePointer, LogItem, LogOptions,
    ProvisionedEntry, RecordQuery, SessionCreateOptions, SessionError, SessionErrorCode,
    SessionMetadata, SessionStats, SessionStorage, SessionTree, UuidIdGenerator,
};
use crate::agent::agent_loop::now_ms;
use crate::agent::types::AgentMessage;

/// 轻量 JSON 校验:遍历序列化值,拒绝非有限数(serde_json 中本不可能出现,
/// 保留以对齐蓝本 `assertJsonSerializable` 的语义边界)。
pub fn assert_json_serializable(value: &Value) -> Result<(), SessionError> {
    let mut stack = vec![value];
    while let Some(current) = stack.pop() {
        match current {
            Value::Number(number) => {
                if let Some(float) = number.as_f64() {
                    if !float.is_finite() {
                        return Err(SessionError::new(
                            SessionErrorCode::InvalidPayload,
                            "Durable payload contains a non-finite number",
                        ));
                    }
                }
            }
            Value::Array(items) => stack.extend(items.iter()),
            Value::Object(fields) => stack.extend(fields.values()),
            _ => {}
        }
    }
    Ok(())
}

fn invalid_query(message: &str) -> SessionError {
    SessionError::new(SessionErrorCode::InvalidQuery, message.to_string())
}

fn assert_valid_limit(limit: Option<usize>) -> Result<(), SessionError> {
    if limit == Some(0) {
        return Err(invalid_query("limit must be a positive integer"));
    }
    Ok(())
}

fn assert_valid_cursor(after_seq: Option<i64>) -> Result<(), SessionError> {
    if let Some(after_seq) = after_seq {
        if after_seq < 0 {
            return Err(invalid_query(
                "cursor sequence must be a non-negative integer",
            ));
        }
    }
    Ok(())
}

/// 会话(对齐 TS `Session implements SessionTree`)。
#[derive(Clone)]
pub struct Session {
    storage: Arc<dyn SessionStorage>,
    id_generator: Arc<dyn IdGenerator>,
}

impl Session {
    pub fn new(storage: Arc<dyn SessionStorage>) -> Self {
        Self::with_id_generator(storage, Arc::new(UuidIdGenerator))
    }

    pub fn with_id_generator(
        storage: Arc<dyn SessionStorage>,
        id_generator: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            storage,
            id_generator,
        }
    }

    /// 底层存储句柄(JsonlSessionStorage 的类型化元数据经具体类型访问)。
    pub fn storage(&self) -> &Arc<dyn SessionStorage> {
        &self.storage
    }

    pub async fn get_metadata(&self) -> Result<SessionMetadata, SessionError> {
        self.storage.get_metadata().await
    }

    /// 返回绑定 lane 的树视图(`main` 即自身)。
    pub fn view(&self, lane: &str) -> Arc<dyn SessionTree> {
        if lane == "main" {
            Arc::new(self.clone())
        } else {
            Arc::new(LaneView {
                session: self.clone(),
                lane: lane.to_string(),
            })
        }
    }

    pub async fn get_lanes(&self) -> Result<Vec<LanePointer>, SessionError> {
        self.storage.get_lanes().await
    }

    pub async fn create_lane(&self, lane: String, at: Option<String>) -> Result<(), SessionError> {
        self.storage.create_lane(lane, at).await
    }

    pub async fn move_lane(&self, lane: String, to: Option<String>) -> Result<(), SessionError> {
        self.storage.move_lane(lane, to).await
    }

    /// 直接追加完整 provisioned 条目(TS `Session.appendEntry`)。
    pub async fn append_entry(
        &self,
        entry: ProvisionedEntry,
        lane: String,
    ) -> Result<Entry, SessionError> {
        self.commit_entry(entry, lane).await
    }

    pub async fn append_record(
        &self,
        record: crate::agent::harness::session::types::LaneRecord,
    ) -> Result<crate::agent::harness::session::types::LaneRecord, SessionError> {
        self.commit_record(record).await
    }

    pub async fn find_records(
        &self,
        query: RecordQuery,
    ) -> Result<Vec<crate::agent::harness::session::types::LaneRecord>, SessionError> {
        self.query_records(query).await
    }

    pub async fn find_open_operations(
        &self,
        lane: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::agent::harness::session::types::OperationStartedRecord>, SessionError>
    {
        assert_valid_limit(limit)?;
        self.storage
            .find_open_operations(lane.to_string(), limit)
            .await
    }

    pub async fn get_log(&self, options: LogOptions) -> Result<Vec<LogItem>, SessionError> {
        self.query_log(options).await
    }

    /// 返回 lane 当前叶,lane 不存在时抛 `invalid_lane`。
    async fn get_leaf_id_for_lane(&self, lane: &str) -> Result<Option<String>, SessionError> {
        let pointer = self
            .get_lanes()
            .await?
            .into_iter()
            .find(|candidate| candidate.lane == lane);
        match pointer {
            Some(pointer) => Ok(pointer.leaf_id),
            None => Err(SessionError::new(
                SessionErrorCode::InvalidLane,
                format!("Lane not found: {lane}"),
            )),
        }
    }

    async fn query_entries(
        &self,
        query: EntryQuery,
        result_limit: Option<usize>,
    ) -> Result<Vec<Entry>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.cursor.map(|cursor| cursor.after_seq))?;
        let effective = if result_limit == query.limit {
            query
        } else {
            EntryQuery {
                limit: result_limit,
                ..query
            }
        };
        self.storage.find_entries(effective).await
    }

    /// 从 query.start 向根查询,缺省从 lane 当前叶开始。
    async fn query_branch_entries(
        &self,
        default_lane: &str,
        query: BranchQuery,
        result_limit: Option<usize>,
    ) -> Result<Vec<Entry>, SessionError> {
        assert_valid_limit(query.query.limit)?;
        assert_valid_cursor(query.query.cursor.map(|cursor| cursor.after_seq))?;
        let start = match query.bounds.start.clone() {
            Some(start) => start,
            None => match self.get_leaf_id_for_lane(default_lane).await? {
                Some(start) => start,
                None => return Ok(Vec::new()),
            },
        };
        let effective_query = if result_limit == query.query.limit {
            query
        } else {
            BranchQuery {
                query: EntryQuery {
                    limit: result_limit,
                    ..query.query
                },
                bounds: query.bounds,
            }
        };
        self.storage
            .find_entries_on_branch(super::types::BranchEntryQuery {
                query: effective_query.query,
                bounds: effective_query.bounds,
                start,
            })
            .await
    }

    async fn query_records(
        &self,
        query: RecordQuery,
    ) -> Result<Vec<crate::agent::harness::session::types::LaneRecord>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.after_seq)?;
        if query.operation_kind.is_some()
            && query.record_type.as_deref() != Some("operation_started")
        {
            return Err(invalid_query(
                "operationKind requires type \"operation_started\"",
            ));
        }
        self.storage.find_records(query).await
    }

    async fn query_log(&self, options: LogOptions) -> Result<Vec<LogItem>, SessionError> {
        assert_valid_limit(options.limit)?;
        assert_valid_cursor(options.after_seq)?;
        self.storage.get_log(options).await
    }

    async fn append_message_to_lane(
        &self,
        lane: &str,
        message: AgentMessage,
    ) -> Result<String, SessionError> {
        let entry = self
            .commit_entry(
                ProvisionedEntry::Message(super::types::ProvisionedMessageEntry {
                    id: self.id_generator.next(),
                    message,
                    terminate: None,
                }),
                lane.to_string(),
            )
            .await?;
        Ok(entry.id().to_string())
    }

    async fn append_custom_entry_to_lane(
        &self,
        lane: &str,
        custom_type: String,
        data: Option<Value>,
    ) -> Result<String, SessionError> {
        let entry = self
            .commit_entry(
                ProvisionedEntry::Custom(super::types::ProvisionedCustomEntry {
                    id: self.id_generator.next(),
                    custom_type,
                    data,
                }),
                lane.to_string(),
            )
            .await?;
        Ok(entry.id().to_string())
    }

    async fn commit_entry(
        &self,
        entry: ProvisionedEntry,
        lane: String,
    ) -> Result<Entry, SessionError> {
        assert_json_serializable(&serde_json::to_value(&entry).map_err(|error| {
            SessionError::new(
                SessionErrorCode::InvalidPayload,
                format!("Durable payload is not serializable: {error}"),
            )
        })?)?;
        self.storage.append_entry(entry, lane).await
    }

    async fn commit_record(
        &self,
        record: crate::agent::harness::session::types::LaneRecord,
    ) -> Result<crate::agent::harness::session::types::LaneRecord, SessionError> {
        assert_json_serializable(&serde_json::to_value(&record).map_err(|error| {
            SessionError::new(
                SessionErrorCode::InvalidPayload,
                format!("Durable payload is not serializable: {error}"),
            )
        })?)?;
        self.storage.append_record(record).await
    }
}

impl SessionTree for Session {
    fn get_leaf_id<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, SessionError>> {
        Box::pin(async move { self.get_leaf_id_for_lane("main").await })
    }

    fn get_entry<'a>(&'a self, id: String) -> BoxFuture<'a, Option<Entry>> {
        Box::pin(async move { self.storage.get_entry(id).await })
    }

    fn get_stats<'a>(&'a self) -> BoxFuture<'a, Result<SessionStats, SessionError>> {
        Box::pin(async move { self.storage.get_stats().await })
    }

    fn get_name<'a>(&'a self) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.storage.get_name().await })
    }

    fn set_name<'a>(&'a self, name: Option<String>) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move { self.storage.set_name(name).await })
    }

    fn get_label<'a>(&'a self, target_id: String) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.storage.get_label(target_id).await })
    }

    fn set_label<'a>(
        &'a self,
        target_id: String,
        label: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move { self.storage.set_label(target_id, label).await })
    }

    fn find_entries<'a>(
        &'a self,
        query: EntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move { self.query_entries(query, None).await })
    }

    fn find_entry<'a>(
        &'a self,
        query: EntryQuery,
    ) -> BoxFuture<'a, Result<Option<Entry>, SessionError>> {
        Box::pin(async move { Ok(self.query_entries(query, Some(1)).await?.into_iter().next()) })
    }

    fn find_entries_on_branch<'a>(
        &'a self,
        query: BranchQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move { self.query_branch_entries("main", query, None).await })
    }

    fn find_entry_on_branch<'a>(
        &'a self,
        query: BranchQuery,
    ) -> BoxFuture<'a, Result<Option<Entry>, SessionError>> {
        Box::pin(async move {
            Ok(self
                .query_branch_entries("main", query, Some(1))
                .await?
                .into_iter()
                .next())
        })
    }

    fn append_message<'a>(
        &'a self,
        message: AgentMessage,
    ) -> BoxFuture<'a, Result<String, SessionError>> {
        Box::pin(async move { self.append_message_to_lane("main", message).await })
    }

    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<Value>,
    ) -> BoxFuture<'a, Result<String, SessionError>> {
        Box::pin(async move {
            self.append_custom_entry_to_lane("main", custom_type, data)
                .await
        })
    }
}

/// 非 main lane 的会话树视图(对齐 TS `Session.view(lane)` 返回的对象)。
#[derive(Clone)]
pub struct LaneView {
    session: Session,
    lane: String,
}

impl SessionTree for LaneView {
    fn get_leaf_id<'a>(&'a self) -> BoxFuture<'a, Result<Option<String>, SessionError>> {
        Box::pin(async move {
            match self.session.get_leaf_id_for_lane(&self.lane).await {
                Ok(leaf_id) => Ok(leaf_id),
                Err(error) => Err(error),
            }
        })
    }

    fn get_entry<'a>(&'a self, id: String) -> BoxFuture<'a, Option<Entry>> {
        Box::pin(async move { self.session.get_entry(id).await })
    }

    fn get_stats<'a>(&'a self) -> BoxFuture<'a, Result<SessionStats, SessionError>> {
        Box::pin(async move { self.session.get_stats().await })
    }

    fn get_name<'a>(&'a self) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.session.get_name().await })
    }

    fn set_name<'a>(&'a self, name: Option<String>) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move { self.session.set_name(name).await })
    }

    fn get_label<'a>(&'a self, target_id: String) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move { self.session.get_label(target_id).await })
    }

    fn set_label<'a>(
        &'a self,
        target_id: String,
        label: Option<String>,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move { self.session.set_label(target_id, label).await })
    }

    fn find_entries<'a>(
        &'a self,
        query: EntryQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        Box::pin(async move { self.session.query_entries(query, None).await })
    }

    fn find_entry<'a>(
        &'a self,
        query: EntryQuery,
    ) -> BoxFuture<'a, Result<Option<Entry>, SessionError>> {
        Box::pin(async move {
            Ok(self
                .session
                .query_entries(query, Some(1))
                .await?
                .into_iter()
                .next())
        })
    }

    fn find_entries_on_branch<'a>(
        &'a self,
        query: BranchQuery,
    ) -> BoxFuture<'a, Result<Vec<Entry>, SessionError>> {
        let lane = self.lane.clone();
        Box::pin(async move { self.session.query_branch_entries(&lane, query, None).await })
    }

    fn find_entry_on_branch<'a>(
        &'a self,
        query: BranchQuery,
    ) -> BoxFuture<'a, Result<Option<Entry>, SessionError>> {
        let lane = self.lane.clone();
        Box::pin(async move {
            Ok(self
                .session
                .query_branch_entries(&lane, query, Some(1))
                .await?
                .into_iter()
                .next())
        })
    }

    fn append_message<'a>(
        &'a self,
        message: AgentMessage,
    ) -> BoxFuture<'a, Result<String, SessionError>> {
        let lane = self.lane.clone();
        Box::pin(async move { self.session.append_message_to_lane(&lane, message).await })
    }

    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<Value>,
    ) -> BoxFuture<'a, Result<String, SessionError>> {
        let lane = self.lane.clone();
        Box::pin(async move {
            self.session
                .append_custom_entry_to_lane(&lane, custom_type, data)
                .await
        })
    }
}

/// 测试/恢复辅助:当前 Unix 毫秒时间戳。
#[doc(hidden)]
pub fn current_timestamp() -> i64 {
    now_ms()
}

/// 会话创建选项的便捷构造(对齐 TS 默认参数形态)。
pub fn create_options() -> SessionCreateOptions {
    SessionCreateOptions::default()
}
