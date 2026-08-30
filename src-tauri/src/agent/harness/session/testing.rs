//! 会话测试设施与内存/JSONL 集成测试:对齐蓝本 `session/testing/`(蓝本为
//! conformance 套件;此处承载离线的 memory + jsonl roundtrip 单测)。

#![cfg(test)]

use crate::agent::agent_loop::testing::test_assistant;
use crate::agent::llm::types::{AssistantContent, StopReason};
use crate::agent::types::{AgentMessage, TypedMessage};
use futures::future::BoxFuture;

use super::memory::{InMemorySessionRepo, InMemorySessionStorage};
use super::session::Session;
use super::types::{
    BranchBounds, BranchQuery, Entry, EntryQuery, ForkOptions, OperationIntent,
    OperationStartedRecord, ProvisionedCustomEntry, ProvisionedEntry, SessionCreateOptions,
    SessionError, SessionErrorCode, SessionStorage, SessionTree, UsageCauseKind, UsageRecord,
};
use crate::agent::harness::types::Result;

fn assistant(text: &str) -> AgentMessage {
    AgentMessage::Message(TypedMessage::Assistant(test_assistant(
        vec![AssistantContent::text(text)],
        StopReason::Stop,
    )))
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::user_text(text, 0)
}

fn make_session() -> Session {
    futures::executor::block_on(async {
        let repo = InMemorySessionRepo::new();
        repo.create(SessionCreateOptions::default()).await.unwrap()
    })
}

#[tokio::test]
async fn appends_messages_and_tracks_stats() {
    let session = make_session();
    let id1 = session.append_message(user("hello")).await.unwrap();
    let id2 = session.append_message(assistant("hi there")).await.unwrap();
    assert_ne!(id1, id2);

    let leaf = session.get_leaf_id().await.unwrap();
    assert_eq!(leaf.as_deref(), Some(id2.as_str()));

    // usage 记录累计统计。
    let mut usage = crate::agent::llm::types::Usage::zero();
    usage.input = 100;
    usage.cache_read = 40;
    usage.cache_write = 10;
    usage.total_tokens = 150;
    usage.cost.total = 0.5;
    session
        .append_record(super::types::LaneRecord::Usage(UsageRecord {
            id: "u-1".into(),
            seq: 0,
            lane: "main".into(),
            timestamp: 0,
            usage,
            cause: UsageCauseKind::Assistant,
            run_id: Some("run-1".into()),
            entry_id: Some(id2.clone()),
            attempt: Some(1),
            stop_reason: Some(StopReason::Stop),
            tool_call_id: None,
            details: None,
        }))
        .await
        .unwrap();
    let stats = session.get_stats().await.unwrap();
    assert_eq!(stats.message_count, 2);
    assert_eq!(stats.cached_tokens, 40);
    assert_eq!(stats.uncached_tokens, 110);
    assert_eq!(stats.total_tokens, 150);
    assert!((stats.cost_total - 0.5).abs() < 1e-9);

    // 名称/标签。
    session.set_name(Some("session-name".into())).await.unwrap();
    assert_eq!(session.get_name().await.as_deref(), Some("session-name"));
    session
        .set_label(id1.clone(), Some("keep".into()))
        .await
        .unwrap();
    assert_eq!(session.get_label(id1.clone()).await.as_deref(), Some("keep"));
}

#[tokio::test]
async fn custom_entries_and_branch_queries() {
    let session = make_session();
    let root = session.append_message(user("root")).await.unwrap();
    let branch_a = session.append_message(assistant("a")).await.unwrap();
    let custom = session
        .append_custom_entry("note".into(), Some(serde_json::json!({"k": 1})))
        .await
        .unwrap();

    let entry = session
        .get_entry(custom.clone())
        .await
        .expect("custom entry exists");
    match &entry {
        Entry::Custom(custom_entry) => {
            assert_eq!(custom_entry.custom_type, "note");
            assert_eq!(custom_entry.data, Some(serde_json::json!({"k": 1})));
        }
        _ => panic!("expected custom entry"),
    }

    // 从叶向根的分支查询。
    let path = session
        .find_entries_on_branch(BranchQuery {
            query: Default::default(),
            bounds: BranchBounds {
                start: Some(custom.clone()),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let ids: Vec<&str> = path.iter().map(|entry| entry.id()).collect();
    assert_eq!(ids, vec![custom.as_str(), branch_a.as_str(), root.as_str()]);

    // 类型过滤。
    let messages = session
        .find_entries(EntryQuery {
            entry_type: Some("message".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn lanes_and_views() {
    let session = make_session();
    let root = session.append_message(user("root")).await.unwrap();
    session
        .create_lane("explore".into(), Some(root.clone()))
        .await
        .unwrap();
    let view = session.view("explore");
    let leaf = view.get_leaf_id().await.unwrap();
    assert_eq!(leaf.as_deref(), Some(root.as_str()));

    // 在侧 lane 追加消息不影响 main 叶。
    let side_leaf = view.append_message(assistant("side")).await.unwrap();
    assert_eq!(
        view.get_leaf_id().await.unwrap().as_deref(),
        Some(side_leaf.as_str())
    );
    assert_eq!(
        session.get_leaf_id().await.unwrap().as_deref(),
        Some(root.as_str())
    );

    // 未知 lane 报错。
    let missing = session.view("nope");
    assert!(matches!(
        missing.get_leaf_id().await,
        Err(SessionError {
            code: SessionErrorCode::InvalidLane,
            ..
        })
    ));

    let lanes = session.get_lanes().await.unwrap();
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].lane, "main");
    assert_eq!(lanes[1].lane, "explore");
}

#[tokio::test]
async fn open_operations_and_fork() {
    let session = make_session();
    session.append_message(user("seed")).await.unwrap();

    // 一个 lane 同时只允许一个 open operation。
    session
        .append_record(super::types::LaneRecord::OperationStarted(
            OperationStartedRecord {
                id: "op-1".into(),
                seq: 0,
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
        ))
        .await
        .unwrap();
    let open = session.find_open_operations("main", None).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, "op-1");

    let second = session
        .append_record(super::types::LaneRecord::OperationStarted(
            OperationStartedRecord {
                id: "op-2".into(),
                seq: 0,
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
        ))
        .await;
    assert!(second.is_err());

    // fork 复制条目并设置 parentSessionId。
    let repo = InMemorySessionRepo::new();
    let parent = repo.create(SessionCreateOptions::default()).await.unwrap();
    parent.append_message(user("inherited")).await.unwrap();
    let parent_id = parent.get_metadata().await.unwrap().id;
    let child = repo
        .fork(
            &parent.get_metadata().await.unwrap(),
            ForkOptions::default(),
            SessionCreateOptions::default(),
        )
        .await
        .unwrap();
    let inherited = child.find_entries(Default::default()).await.unwrap();
    assert_eq!(inherited.len(), 1);
    match &inherited[0] {
        Entry::Message(message_entry) => {
            assert_eq!(message_entry.message.role_name(), "user");
        }
        _ => panic!("expected message entry"),
    }
    let metadata = child.get_metadata().await.unwrap();
    assert_eq!(metadata.parent_session_id.as_deref(), Some(parent_id.as_str()));
}

// ---------------------------------------------------------------------------
// JSONL 存储往返(内存文件系统)
// ---------------------------------------------------------------------------

/// 内存文件系统:HashMap 后端(离线)。
#[derive(Default)]
struct MemFs {
    files: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    counter: std::sync::atomic::AtomicU64,
}

impl crate::agent::harness::types::FileSystem for MemFs {
    fn cwd(&self) -> &str {
        "/"
    }

    fn absolute_path<'a>(
        &'a self,
        path: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
        Box::pin(async move { Ok(path) })
    }

    fn join_path<'a>(
        &'a self,
        parts: Vec<String>,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
        Box::pin(async move { Ok(parts.join("/")) })
    }

    fn read_text_file<'a>(
        &'a self,
        path: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let files = self.files.lock().unwrap();
            files
                .get(&path)
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .ok_or_else(|| {
                    crate::agent::harness::types::FileError::new(
                        crate::agent::harness::types::FileErrorCode::NotFound,
                        "missing",
                    )
                })
        })
    }

    fn read_text_lines<'a>(
        &'a self,
        path: String,
        options: crate::agent::harness::types::ReadTextLinesOptions,
    ) -> BoxFuture<'a, Result<Vec<String>, crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let files = self.files.lock().unwrap();
            let content = files
                .get(&path)
                .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                .ok_or_else(|| {
                    crate::agent::harness::types::FileError::new(
                        crate::agent::harness::types::FileErrorCode::NotFound,
                        "missing",
                    )
                })?;
            let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
            if content.ends_with('\n') {
                lines.pop();
            }
            if let Some(max_lines) = options.max_lines {
                lines.truncate(max_lines);
            }
            Ok(lines)
        })
    }

    fn read_binary_file<'a>(
        &'a self,
        path: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<u8>, crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let files = self.files.lock().unwrap();
            files.get(&path).cloned().ok_or_else(|| {
                crate::agent::harness::types::FileError::new(
                    crate::agent::harness::types::FileErrorCode::NotFound,
                    "missing",
                )
            })
        })
    }

    fn write_file<'a>(
        &'a self,
        path: String,
        content: crate::agent::harness::types::FileContent,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let bytes = match content {
                crate::agent::harness::types::FileContent::Text(text) => text.into_bytes(),
                crate::agent::harness::types::FileContent::Binary(bytes) => bytes,
            };
            self.files.lock().unwrap().insert(path, bytes);
            Ok(())
        })
    }

    fn append_file<'a>(
        &'a self,
        path: String,
        content: crate::agent::harness::types::FileContent,
    ) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let bytes = match content {
                crate::agent::harness::types::FileContent::Text(text) => text.into_bytes(),
                crate::agent::harness::types::FileContent::Binary(bytes) => bytes,
            };
            let mut files = self.files.lock().unwrap();
            files.entry(path).or_default().extend(bytes);
            Ok(())
        })
    }

    fn rename_file<'a>(
        &'a self,
        source: String,
        destination: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let mut files = self.files.lock().unwrap();
            if let Some(bytes) = files.remove(&source) {
                files.insert(destination, bytes);
                Ok(())
            } else {
                Err(crate::agent::harness::types::FileError::new(
                    crate::agent::harness::types::FileErrorCode::NotFound,
                    "missing",
                ))
            }
        })
    }

    fn file_info<'a>(
        &'a self,
        path: String,
    ) -> BoxFuture<'a, Result<crate::agent::harness::types::FileInfo, crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            let files = self.files.lock().unwrap();
            files
                .get(&path)
                .map(|bytes| crate::agent::harness::types::FileInfo {
                    name: path.clone(),
                    path,
                    kind: crate::agent::harness::types::FileKind::File,
                    size: bytes.len() as u64,
                    mtime_ms: 0.0,
                })
                .ok_or_else(|| {
                    crate::agent::harness::types::FileError::new(
                        crate::agent::harness::types::FileErrorCode::NotFound,
                        "missing",
                    )
                })
        })
    }

    fn list_dir<'a>(
        &'a self,
        _path: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<crate::agent::harness::types::FileInfo>, crate::agent::harness::types::FileError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn canonical_path<'a>(
        &'a self,
        path: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
        Box::pin(async move { Ok(path) })
    }

    fn exists<'a>(
        &'a self,
        path: String,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<bool, crate::agent::harness::types::FileError>> {
        Box::pin(async move { Ok(self.files.lock().unwrap().contains_key(&path)) })
    }

    fn create_dir<'a>(
        &'a self,
        _path: String,
        _options: crate::agent::harness::types::CreateDirOptions,
    ) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
        Box::pin(async move { Ok(()) })
    }

    fn remove<'a>(
        &'a self,
        path: String,
        _options: crate::agent::harness::types::RemoveOptions,
    ) -> BoxFuture<'a, Result<(), crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            self.files.lock().unwrap().remove(&path);
            Ok(())
        })
    }

    fn create_temp_dir<'a>(
        &'a self,
        _prefix: Option<String>,
        _abort: Option<crate::agent::types::AbortSignal>,
    ) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            Ok(format!(
                "/tmp/{}",
                self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ))
        })
    }

    fn create_temp_file<'a>(
        &'a self,
        _options: crate::agent::harness::types::CreateTempFileOptions,
    ) -> BoxFuture<'a, Result<String, crate::agent::harness::types::FileError>> {
        Box::pin(async move {
            Ok(format!(
                "/tmp/file-{}",
                self.counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ))
        })
    }

    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {})
    }
}

impl crate::agent::harness::types::Shell for MemFs {
    fn exec<'a>(
        &'a self,
        _command: String,
        _options: crate::agent::harness::types::ShellExecOptions,
    ) -> BoxFuture<'a, Result<crate::agent::harness::types::ExecOutcome, crate::agent::harness::types::ExecutionError>> {
        Box::pin(async move {
            Ok(crate::agent::harness::types::ExecOutcome {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        })
    }

    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async move {})
    }
}

#[tokio::test]
async fn jsonl_storage_round_trip_via_memory_fs() {
    use crate::agent::harness::session::jsonl::{JsonlSessionStorage, JsonlV4Header};

    let fs: std::sync::Arc<dyn crate::agent::harness::types::FileSystem> =
        std::sync::Arc::new(MemFs::default());
    let header = JsonlV4Header::new("session-1", 1_700_000_000_000, "/workspace");
    let path = "/workspace/--workspace--/session.jsonl";
    let storage = JsonlSessionStorage::create(fs.clone(), path, header)
        .await
        .unwrap();

    // 追加条目与记录后重载,状态一致。
    let appended = storage
        .append_entry(
            ProvisionedEntry::Custom(ProvisionedCustomEntry {
                id: "e-1".into(),
                custom_type: "note".into(),
                data: Some(serde_json::json!({"n": 1})),
            }),
            "main".into(),
        )
        .await
        .unwrap();
    assert_eq!(appended.seq(), 1);
    storage
        .append_record(super::types::LaneRecord::OperationStarted(
            OperationStartedRecord {
                id: "op-1".into(),
                seq: 0,
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
        ))
        .await
        .unwrap();
    storage.set_name(Some("named".into())).await.unwrap();
    storage.drain().await;

    let reloaded = JsonlSessionStorage::load(fs, path).await.unwrap();
    assert_eq!(reloaded.get_name().await, Some("named".to_string()));
    let entry = reloaded.get_entry("e-1".into()).await.expect("entry e-1 exists");
    assert_eq!(entry.seq(), 1);
    assert!(matches!(entry, Entry::Custom(_)));
    let open = reloaded
        .find_open_operations("main".into(), None)
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    let stats = reloaded.get_stats().await.unwrap();
    assert_eq!(stats.message_count, 0);
}

/// InMemorySessionStorage 也参与 Session 门面(编译期契约检查)。
#[test]
fn session_facade_over_memory_storage() {
    let storage = std::sync::Arc::new(InMemorySessionStorage::new(super::types::SessionMetadata {
        id: "s-1".into(),
        created_at: 0,
        parent_session_id: None,
    }));
    let session = Session::new(storage);
    futures::executor::block_on(async {
        session.append_message(user("hello")).await.unwrap();
        assert_eq!(session.get_stats().await.unwrap().message_count, 1);
    });
}
