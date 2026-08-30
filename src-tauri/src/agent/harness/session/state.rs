//! 会话状态机:对齐 `packages/agent/src/harness/session/state.ts`。
//!
//! `SessionState` 以严格 seq 递增的方式应用 [`SessionMutation`],维护
//! entries/records/lane 指针/全局事实/log/统计。查询逻辑与 TS 逐行对齐。

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{
    BranchBounds, Entry, EntryOrder, EntryQuery, ForkOptions, ForkPosition, ForkScope, LanePointer,
    LaneRecord, LogItem, OperationStartedRecord, SessionError, SessionErrorCode, SessionFact,
    SessionStats,
};

/// 会话变更(对齐 TS `SessionMutation`)。
///
/// serde 为手工实现:TS 的 entry/record 变更把载荷字段平铺到 `{kind: ...}` 对象
/// (`{kind:"entry", lane?, ...entry}`),Rust 端经 `serde_json::Value` 组合/拆解,
/// 保证行格式与蓝本 JSONL 完全兼容。
#[derive(Clone, Debug, PartialEq)]
pub enum SessionMutation {
    Entry {
        lane: Option<String>,
        entry: Entry,
    },
    Record {
        record: LaneRecord,
    },
    Lane {
        seq: i64,
        lane: String,
        leaf_id: Option<String>,
    },
    Fact {
        seq: i64,
        fact: SessionFact,
    },
}

impl Serialize for SessionMutation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serde_json::Map::new();
        match self {
            SessionMutation::Entry { lane, entry } => {
                map.insert("kind".to_string(), Value::String("entry".to_string()));
                if let Some(lane) = lane {
                    map.insert("lane".to_string(), Value::String(lane.clone()));
                }
                let entry_value = serde_json::to_value(entry).map_err(serde::ser::Error::custom)?;
                let Value::Object(entry_fields) = entry_value else {
                    return Err(serde::ser::Error::custom("entry must serialize to an object"));
                };
                for (key, value) in entry_fields {
                    map.insert(key, value);
                }
            }
            SessionMutation::Record { record } => {
                map.insert("kind".to_string(), Value::String("record".to_string()));
                let record_value = serde_json::to_value(record).map_err(serde::ser::Error::custom)?;
                let Value::Object(record_fields) = record_value else {
                    return Err(serde::ser::Error::custom("record must serialize to an object"));
                };
                for (key, value) in record_fields {
                    map.insert(key, value);
                }
            }
            SessionMutation::Lane { seq, lane, leaf_id } => {
                map.insert("kind".to_string(), Value::String("lane".to_string()));
                map.insert("seq".to_string(), Value::from(*seq));
                map.insert("lane".to_string(), Value::String(lane.clone()));
                map.insert(
                    "leafId".to_string(),
                    leaf_id
                        .as_ref()
                        .map(|id| Value::String(id.clone()))
                        .unwrap_or(Value::Null),
                );
            }
            SessionMutation::Fact { seq, fact } => {
                map.insert("kind".to_string(), Value::String("fact".to_string()));
                map.insert("seq".to_string(), Value::from(*seq));
                let fact_value = serde_json::to_value(fact).map_err(serde::ser::Error::custom)?;
                let Value::Object(fact_fields) = fact_value else {
                    return Err(serde::ser::Error::custom("fact must serialize to an object"));
                };
                for (key, value) in fact_fields {
                    map.insert(key, value);
                }
            }
        }
        Value::Object(map).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SessionMutation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        from_value(value).map_err(serde::de::Error::custom)
    }
}

impl SessionMutation {
    pub fn seq(&self) -> i64 {
        match self {
            SessionMutation::Entry { entry, .. } => entry.seq(),
            SessionMutation::Record { record } => record.seq(),
            SessionMutation::Lane { seq, .. } => *seq,
            SessionMutation::Fact { seq, .. } => *seq,
        }
    }
}

/// 从平铺 JSON 对象解析 `SessionMutation`(与 codec 的解码严格度一致)。
pub fn session_mutation_from_value(value: Value) -> Result<SessionMutation, SessionError> {
    from_value(value)
}

fn invalid<S: Into<String>>(message: S) -> SessionError {
    SessionError::new(
        SessionErrorCode::InvalidEntry,
        format!("Invalid session mutation: {}", message.into()),
    )
}

fn from_value(value: Value) -> Result<SessionMutation, SessionError> {
    let Value::Object(map) = value else {
        return Err(invalid("is not a JSON object"));
    };
    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("is missing kind"))?;
    match kind {
        "entry" => {
            let lane = match map.get("lane") {
                None | Some(Value::Null) => None,
                Some(Value::String(lane)) => Some(lane.clone()),
                Some(_) => return Err(invalid("has invalid lane")),
            };
            let entry_value = {
                let mut fields = map.clone();
                fields.remove("kind");
                fields.remove("lane");
                Value::Object(fields)
            };
            let entry: Entry = serde_json::from_value(entry_value)
                .map_err(|error| invalid(format!("has invalid entry payload: {error}")))?;
            Ok(SessionMutation::Entry { lane, entry })
        }
        "record" => {
            let record_value = {
                let mut fields = map.clone();
                fields.remove("kind");
                Value::Object(fields)
            };
            let record: LaneRecord = serde_json::from_value(record_value)
                .map_err(|error| invalid(format!("has invalid record payload: {error}")))?;
            Ok(SessionMutation::Record { record })
        }
        "lane" => {
            let seq = require_seq(&map)?;
            let lane = require_string(&map, "lane")?;
            let leaf_id = match map.get("leafId") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) => Some(id.clone()),
                Some(_) => return Err(invalid("has invalid leafId")),
            };
            Ok(SessionMutation::Lane { seq, lane, leaf_id })
        }
        "fact" => {
            let seq = require_seq(&map)?;
            match map.get("fact").and_then(Value::as_str) {
                Some("name") => {
                    let name = match map.get("name") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(name)) => Some(name.clone()),
                        Some(_) => return Err(invalid("has invalid name")),
                    };
                    Ok(SessionMutation::Fact {
                        seq,
                        fact: SessionFact::Name { name },
                    })
                }
                Some("label") => {
                    let target_id = require_string(&map, "targetId")?;
                    let label = match map.get("label") {
                        None | Some(Value::Null) => None,
                        Some(Value::String(label)) => Some(label.clone()),
                        Some(_) => return Err(invalid("has invalid label")),
                    };
                    Ok(SessionMutation::Fact {
                        seq,
                        fact: SessionFact::Label { target_id, label },
                    })
                }
                _ => Err(invalid("has unknown fact type")),
            }
        }
        _ => Err(invalid("has unknown mutation kind")),
    }
}

fn require_seq(map: &serde_json::Map<String, Value>) -> Result<i64, SessionError> {
    match map.get("seq") {
        Some(Value::Number(number)) => number
            .as_i64()
            .ok_or_else(|| invalid("has invalid seq")),
        _ => Err(invalid("has invalid seq")),
    }
}

fn require_string(
    map: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, SessionError> {
    match map.get(field) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(invalid(format!("has invalid {field}"))),
    }
}

// ---------------------------------------------------------------------------
// 插入序 Map(TS Map 保序;Rust 用 Vec 承载)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct OrderedMap<V> {
    items: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<V> OrderedMap<V> {
    fn get(&self, key: &str) -> Option<&V> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn contains_key(&self, key: &str) -> bool {
        self.items.iter().any(|(k, _)| k == key)
    }

    fn insert(&mut self, key: impl Into<String>, value: V) {
        let key = key.into();
        if let Some(slot) = self.items.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            self.items.push((key, value));
        }
    }

    fn remove(&mut self, key: &str) -> Option<V> {
        let index = self.items.iter().position(|(k, _)| k == key)?;
        Some(self.items.remove(index).1)
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.items.iter().map(|(k, v)| (k, v))
    }
}

/// 纯内存会话状态(对齐 TS `SessionState`)。
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    sequence: i64,
    used_ids: HashSet<String>,
    entries: Vec<Entry>,
    entries_by_id: HashMap<String, Entry>,
    records: Vec<LaneRecord>,
    /// lane → (operationId → record),插入序。
    open_operations_by_lane: HashMap<String, OrderedMap<OperationStartedRecord>>,
    /// lane → leafId,插入序(首项固定 main)。
    lanes: OrderedMap<Option<String>>,
    log: Vec<LogItem>,
    stats: SessionStats,
    name: Option<String>,
    labels: HashMap<String, String>,
}

impl SessionState {
    pub fn new() -> Self {
        let mut lanes = OrderedMap::default();
        lanes.insert("main", None);
        Self {
            sequence: 0,
            used_ids: HashSet::new(),
            entries: Vec::new(),
            entries_by_id: HashMap::new(),
            records: Vec::new(),
            open_operations_by_lane: HashMap::new(),
            lanes,
            log: Vec::new(),
            stats: SessionStats::default(),
            name: None,
            labels: HashMap::new(),
        }
    }

    pub fn next_sequence(&self) -> i64 {
        self.sequence + 1
    }

    pub fn get_lanes(&self) -> Vec<LanePointer> {
        self.lanes
            .iter()
            .map(|(lane, leaf_id)| LanePointer {
                lane: lane.clone(),
                leaf_id: leaf_id.clone(),
            })
            .collect()
    }

    /// 返回 lane 当前叶;lane 不存在时抛 `invalid_lane`。
    pub fn require_lane(&self, lane: &str) -> Result<Option<String>, SessionError> {
        self.lanes.get(lane).cloned().ok_or_else(|| {
            SessionError::new(
                SessionErrorCode::InvalidLane,
                format!("Lane not found: {lane}"),
            )
        })
    }

    /// 新建 lane 前校验:不得已存在。
    pub fn validate_new_lane(&self, lane: &str) -> Result<(), SessionError> {
        if self.lanes.contains_key(lane) {
            return Err(SessionError::new(
                SessionErrorCode::AlreadyExists,
                format!("Lane already exists: {lane}"),
            ));
        }
        Ok(())
    }

    /// 校验目标条目存在(null 合法)。
    pub fn validate_target(&self, target_id: Option<&str>) -> Result<(), SessionError> {
        if let Some(target_id) = target_id {
            if !self.entries_by_id.contains_key(target_id) {
                return Err(SessionError::new(
                    SessionErrorCode::NotFound,
                    format!("Entry not found: {target_id}"),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_unused_id(&self, id: &str) -> Result<(), SessionError> {
        if self.used_ids.contains(id) {
            return Err(SessionError::new(
                SessionErrorCode::AlreadyExists,
                format!("Session id already exists: {id}"),
            ));
        }
        Ok(())
    }

    /// 应用一条变更;非连续 seq / 重复 id / 断链等都会抛 `invalid_entry`。
    pub fn apply_mutation(&mut self, mutation: &SessionMutation) -> Result<(), SessionError> {
        let seq = mutation.seq();
        if seq != self.sequence + 1 {
            return Err(invalid(format!("has non-consecutive seq {seq}")));
        }

        match mutation {
            SessionMutation::Entry { lane, entry } => {
                if self.used_ids.contains(entry.id()) {
                    return Err(invalid(format!("contains duplicate id {}", entry.id())));
                }
                if let Some(lane) = lane {
                    let leaf_id = match self.lanes.get(lane) {
                        Some(leaf_id) => leaf_id.clone(),
                        None => return Err(invalid(format!("references missing lane {lane}"))),
                    };
                    if entry.parent_id().map(str::to_string) != leaf_id {
                        return Err(invalid("does not chain to the lane leaf"));
                    }
                }
                if let Some(parent_id) = entry.parent_id() {
                    if !self.entries_by_id.contains_key(parent_id) {
                        return Err(invalid(format!("references missing parent {parent_id}")));
                    }
                }
                self.sequence = seq;
                self.used_ids.insert(entry.id().to_string());
                self.entries.push(entry.clone());
                self.entries_by_id.insert(entry.id().to_string(), entry.clone());
                if let Some(lane) = lane {
                    self.lanes.insert(lane.clone(), Some(entry.id().to_string()));
                }
                self.log.push(LogItem::Entry {
                    seq,
                    entry: entry.clone(),
                });
                if let Entry::Message(_) = entry {
                    self.stats.message_count += 1;
                }
            }
            SessionMutation::Record { record } => {
                if !self.lanes.contains_key(record.lane()) {
                    return Err(invalid(format!("references missing lane {}", record.lane())));
                }
                if self.used_ids.contains(record.id()) {
                    return Err(invalid(format!("contains duplicate id {}", record.id())));
                }
                self.sequence = seq;
                self.used_ids.insert(record.id().to_string());
                self.records.push(record.clone());
                if let LaneRecord::OperationStarted(operation) = record {
                    self.open_operations_by_lane
                        .entry(operation.lane.clone())
                        .or_default()
                        .insert(operation.id.clone(), operation.clone());
                } else if let LaneRecord::OperationFinished(finished) = record {
                    if let Some(open) = self.open_operations_by_lane.get_mut(&finished.lane) {
                        open.remove(&finished.run_id);
                    }
                }
                self.log.push(LogItem::Record {
                    seq,
                    record: record.clone(),
                });
                if let LaneRecord::Usage(usage) = record {
                    self.stats.cached_tokens += usage.usage.cache_read;
                    self.stats.uncached_tokens += usage.usage.input + usage.usage.cache_write;
                    self.stats.total_tokens += usage.usage.total_tokens;
                    self.stats.cost_total += usage.usage.cost.total;
                }
            }
            SessionMutation::Lane { lane, leaf_id, .. } => {
                if let Some(leaf_id) = leaf_id {
                    if !self.entries_by_id.contains_key(leaf_id) {
                        return Err(invalid(format!("references missing lane target {leaf_id}")));
                    }
                }
                self.sequence = seq;
                self.lanes.insert(lane.clone(), leaf_id.clone());
                self.log.push(LogItem::Lane {
                    seq,
                    lane: lane.clone(),
                    leaf_id: leaf_id.clone(),
                });
            }
            SessionMutation::Fact { fact, .. } => match fact {
                SessionFact::Label { target_id, label } => {
                    if !self.entries_by_id.contains_key(target_id) {
                        return Err(invalid(format!("references missing label target {target_id}")));
                    }
                    self.sequence = seq;
                    match label {
                        Some(label) => {
                            self.labels.insert(target_id.clone(), label.clone());
                        }
                        None => {
                            self.labels.remove(target_id);
                        }
                    }
                    self.log.push(LogItem::Fact {
                        seq,
                        fact: fact.clone(),
                    });
                }
                SessionFact::Name { name } => {
                    self.sequence = seq;
                    self.name = name.clone();
                    self.log.push(LogItem::Fact {
                        seq,
                        fact: fact.clone(),
                    });
                }
            },
        }
        Ok(())
    }

    pub fn get_entry(&self, id: &str) -> Option<&Entry> {
        self.entries_by_id.get(id)
    }

    pub fn find_entries(&self, query: &EntryQuery) -> Result<Vec<Entry>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.cursor.map(|cursor| cursor.after_seq))?;
        let mut results = Vec::new();
        for entry in ordered(&self.entries, query.order) {
            if !self.matches_entry_query(&entry, query) {
                continue;
            }
            results.push(entry.clone());
            if query.limit.is_some() && results.len() == query.limit.unwrap() {
                break;
            }
        }
        Ok(results)
    }

    /// 存储层分支查询(start 必填)。
    pub fn find_entries_on_branch(
        &self,
        entry_query: &EntryQuery,
        bounds: &BranchBounds,
        start: &str,
    ) -> Result<Vec<Entry>, SessionError> {
        assert_valid_limit(entry_query.limit)?;
        assert_valid_cursor(entry_query.cursor.map(|cursor| cursor.after_seq))?;
        let mut results = Vec::new();
        if entry_query.order == Some(EntryOrder::OldestFirst) {
            let path: Vec<Entry> = self.walk_to_root(Some(start), &BranchBounds::default())?;
            for entry in path.iter().rev() {
                let reached_bound = Some(entry.id()) == bounds.stop_at_id.as_deref()
                    || Some(entry.entry_type().to_string()) == bounds.stop_at_type;
                if self.matches_entry_query(entry, entry_query) {
                    results.push(entry.clone());
                }
                if reached_bound || (entry_query.limit.is_some() && results.len() == entry_query.limit.unwrap()) {
                    break;
                }
            }
        } else {
            for entry in self.walk_to_root(Some(start), bounds)? {
                if self.matches_entry_query(&entry, entry_query) {
                    results.push(entry);
                }
                if entry_query.limit.is_some() && results.len() == entry_query.limit.unwrap() {
                    break;
                }
            }
        }
        Ok(results)
    }

    pub fn find_records(&self, query: &super::types::RecordQuery) -> Result<Vec<LaneRecord>, SessionError> {
        assert_valid_limit(query.limit)?;
        assert_valid_cursor(query.after_seq)?;
        let mut results = Vec::new();
        for record in ordered(&self.records, query.order) {
            if !self.matches_record_query(&record, query) {
                continue;
            }
            results.push(record.clone());
            if query.limit.is_some() && results.len() == query.limit.unwrap() {
                break;
            }
        }
        Ok(results)
    }

    pub fn find_open_operations(
        &self,
        lane: &str,
        limit: Option<usize>,
    ) -> Result<Vec<OperationStartedRecord>, SessionError> {
        assert_valid_limit(limit)?;
        let mut open_operations: Vec<OperationStartedRecord> = self
            .open_operations_by_lane
            .get(lane)
            .map(|map| map.iter().map(|(_, record)| record.clone()).collect())
            .unwrap_or_default();
        open_operations.reverse();
        Ok(match limit {
            Some(limit) => open_operations.into_iter().take(limit).collect(),
            None => open_operations,
        })
    }

    pub fn get_log(&self, options: &super::types::LogOptions) -> Result<Vec<LogItem>, SessionError> {
        assert_valid_limit(options.limit)?;
        assert_valid_cursor(options.after_seq)?;
        let mut results = Vec::new();
        for item in &self.log {
            if let Some(after_seq) = options.after_seq {
                if item.seq() <= after_seq {
                    continue;
                }
            }
            results.push(item.clone());
            if options.limit.is_some() && results.len() == options.limit.unwrap() {
                break;
            }
        }
        Ok(results)
    }

    pub fn get_name(&self) -> Option<String> {
        self.name.clone()
    }

    pub fn get_label(&self, id: &str) -> Option<String> {
        self.labels.get(id).cloned()
    }

    pub fn get_stats(&self) -> SessionStats {
        self.stats
    }

    /// 生成 fork 变更序列(对齐 TS `createForkMutations`)。
    pub fn create_fork_mutations(&self, options: &ForkOptions) -> Result<Vec<SessionMutation>, SessionError> {
        let copied_entries: Vec<Entry>;
        let fork_lanes: Vec<LanePointer>;
        if options.scope == Some(ForkScope::Tree) {
            copied_entries = self.find_entries(&EntryQuery {
                order: Some(EntryOrder::OldestFirst),
                ..Default::default()
            })?;
            fork_lanes = self.get_lanes();
        } else {
            let selected_entry_id = self.require_lane("main")?;
            let mut target_id: Option<String> = None;
            if let Some(selected_entry_id) = selected_entry_id {
                let entry = self
                    .get_entry(&selected_entry_id)
                    .ok_or_else(|| {
                        SessionError::new(
                            SessionErrorCode::InvalidForkTarget,
                            format!("Fork target is not a message entry: {selected_entry_id}"),
                        )
                    })?;
                if entry.entry_type() != "message" {
                    return Err(SessionError::new(
                        SessionErrorCode::InvalidForkTarget,
                        format!("Fork target is not a message entry: {selected_entry_id}"),
                    ));
                }
                let position = options.position.unwrap_or(if options.entry_id.is_none() {
                    ForkPosition::At
                } else {
                    ForkPosition::Before
                });
                target_id = match position {
                    ForkPosition::At => entry.id().to_string().into(),
                    ForkPosition::Before => entry.parent_id().map(str::to_string),
                };
            }
            copied_entries = match &target_id {
                None => Vec::new(),
                Some(target_id) => self.find_entries_on_branch(
                    &EntryQuery {
                        order: Some(EntryOrder::OldestFirst),
                        ..Default::default()
                    },
                    &BranchBounds::default(),
                    target_id,
                )?,
            };
            fork_lanes = vec![LanePointer {
                lane: "main".to_string(),
                leaf_id: target_id,
            }];
        }

        let mut mutations: Vec<SessionMutation> = Vec::new();
        let mut sequence: i64 = 1;
        for source_entry in &copied_entries {
            let mut cloned = source_entry.clone();
            set_entry_seq(&mut cloned, sequence);
            sequence += 1;
            mutations.push(SessionMutation::Entry {
                lane: None,
                entry: cloned,
            });
        }
        for pointer in &fork_lanes {
            mutations.push(SessionMutation::Lane {
                seq: sequence,
                lane: pointer.lane.clone(),
                leaf_id: pointer.leaf_id.clone(),
            });
            sequence += 1;
        }
        if let Some(name) = &self.name {
            mutations.push(SessionMutation::Fact {
                seq: sequence,
                fact: SessionFact::Name {
                    name: Some(name.clone()),
                },
            });
            sequence += 1;
        }
        for entry in &copied_entries {
            if let Some(label) = self.labels.get(entry.id()) {
                mutations.push(SessionMutation::Fact {
                    seq: sequence,
                    fact: SessionFact::Label {
                        target_id: entry.id().to_string(),
                        label: Some(label.clone()),
                    },
                });
                sequence += 1;
            }
        }
        Ok(mutations)
    }

    /// 从 start 向根遍历;环与缺失父条目都会抛 `invalid_entry`/`not_found`。
    fn walk_to_root(
        &self,
        start: Option<&str>,
        bounds: &BranchBounds,
    ) -> Result<Vec<Entry>, SessionError> {
        let Some(current_id) = start else {
            return Ok(Vec::new());
        };
        let mut visited = HashSet::new();
        let mut current = self.entries_by_id.get(current_id).cloned().ok_or_else(|| {
            SessionError::new(
                SessionErrorCode::NotFound,
                format!("Entry not found: {current_id}"),
            )
        })?;
        let mut path = Vec::new();
        loop {
            if visited.contains(current.id()) {
                return Err(SessionError::new(
                    SessionErrorCode::InvalidEntry,
                    format!("Session branch contains a cycle at {}", current.id()),
                ));
            }
            visited.insert(current.id().to_string());
            path.push(current.clone());
            if Some(current.id()) == bounds.stop_at_id.as_deref()
                || Some(current.entry_type().to_string()) == bounds.stop_at_type
                || current.parent_id().is_none()
            {
                break;
            }
            let parent_id = current.parent_id().unwrap().to_string();
            current = self.entries_by_id.get(&parent_id).cloned().ok_or_else(|| {
                SessionError::new(
                    SessionErrorCode::InvalidEntry,
                    format!("Entry not found: {parent_id}"),
                )
            })?;
        }
        Ok(path)
    }

    fn matches_entry_query(&self, entry: &Entry, query: &EntryQuery) -> bool {
        let type_matches = query
            .entry_type
            .as_ref()
            .map(|t| entry.entry_type() == t.as_str())
            .unwrap_or(true);
        let custom_matches = query
            .custom_type
            .as_ref()
            .map(|custom| match entry {
                Entry::Custom(custom_entry) => &custom_entry.custom_type == custom,
                _ => false,
            })
            .unwrap_or(true);
        let cursor_matches = query
            .cursor
            .map(|cursor| {
                if query.order == Some(EntryOrder::OldestFirst) {
                    entry.seq() > cursor.after_seq
                } else {
                    entry.seq() < cursor.after_seq
                }
            })
            .unwrap_or(true);
        type_matches && custom_matches && cursor_matches
    }

    fn matches_record_query(
        &self,
        record: &LaneRecord,
        query: &super::types::RecordQuery,
    ) -> bool {
        let lane_matches = query
            .lane
            .as_ref()
            .map(|lane| record.lane() == lane.as_str())
            .unwrap_or(true);
        let type_matches = query
            .record_type
            .as_ref()
            .map(|t| record.record_type() == t.as_str())
            .unwrap_or(true);
        let run_matches = query
            .run_id
            .as_ref()
            .map(|run_id| record.operation_identity() == Some(run_id.as_str()))
            .unwrap_or(true);
        let operation_kind_matches = query
            .operation_kind
            .as_ref()
            .map(|kind| {
                match record {
                    LaneRecord::OperationStarted(operation) => {
                        serde_json::to_value(&operation.intent)
                            .ok()
                            .and_then(|value| {
                                value
                                    .get("kind")
                                    .and_then(Value::as_str)
                                    .map(|k| k == kind.as_str())
                            })
                            .unwrap_or(false)
                    }
                    _ => false,
                }
            })
            .unwrap_or(true);
        let after_seq_matches = query
            .after_seq
            .map(|after_seq| record.seq() > after_seq)
            .unwrap_or(true);
        lane_matches && type_matches && run_matches && operation_kind_matches && after_seq_matches
    }
}

fn set_entry_seq(entry: &mut Entry, seq: i64) {
    match entry {
        Entry::Message(e) => e.seq = seq,
        Entry::ModelChange(e) => e.seq = seq,
        Entry::ThinkingLevelChange(e) => e.seq = seq,
        Entry::ActiveToolsChange(e) => e.seq = seq,
        Entry::Compaction(e) => e.seq = seq,
        Entry::BranchSummary(e) => e.seq = seq,
        Entry::Custom(e) => e.seq = seq,
    }
}

fn assert_valid_limit(limit: Option<usize>) -> Result<(), SessionError> {
    if let Some(limit) = limit {
        if limit == 0 {
            return Err(SessionError::new(
                SessionErrorCode::InvalidQuery,
                "limit must be a positive integer",
            ));
        }
    }
    Ok(())
}

fn assert_valid_cursor(after_seq: Option<i64>) -> Result<(), SessionError> {
    if let Some(after_seq) = after_seq {
        if after_seq < 0 {
            return Err(SessionError::new(
                SessionErrorCode::InvalidQuery,
                "cursor sequence must be a non-negative integer",
            ));
        }
    }
    Ok(())
}

/// TS `ordered`:oldestFirst 按序,否则倒序。
fn ordered<T: Clone>(items: &[T], order: Option<EntryOrder>) -> impl Iterator<Item = T> {
    let items = items.to_vec();
    match order {
        Some(EntryOrder::OldestFirst) => itertools_like(items, false),
        _ => itertools_like(items, true),
    }
}

fn itertools_like<T: Clone>(items: Vec<T>, reverse: bool) -> impl Iterator<Item = T> {
    let indices: Vec<usize> = if reverse {
        (0..items.len()).rev().collect()
    } else {
        (0..items.len()).collect()
    };
    indices.into_iter().map(move |index| items[index].clone())
}
