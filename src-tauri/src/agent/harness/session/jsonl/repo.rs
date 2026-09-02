//! JSONL 会话仓库:对齐 `packages/agent/src/harness/session/jsonl/repo.ts`。
//!
//! 会话文件落在 `<sessionsRoot>/<--cwd 编码-->/<ISO 时间戳>_<id>.jsonl`;
//! 同进程的 create/fork 目的地经注册表互斥(对齐 TS `claimCreateDestination`)。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chrono::{SecondsFormat, TimeZone};
use regex::Regex;

use super::codec::{metadata_from_header, parse_header};
use super::errors::file_result;
use super::storage::JsonlSessionStorage;
use super::types::{
    JsonlSessionCreateOptions, JsonlSessionListOptions, JsonlSessionMetadata,
    JsonlSessionRepoOptions, JsonlV4Header,
};
use crate::agent::agent_loop::now_ms;
use crate::agent::harness::session::session::Session;
use crate::agent::harness::session::types::{ForkOptions, SessionError, SessionErrorCode};
use crate::agent::harness::types::{CreateDirOptions, FileKind, FileSystem};
use crate::agent::harness::uuid::uuid_v7;

/// 会话 id 模式:非空,仅字母数字/-/_/.,首尾为字母数字。
fn session_id_pattern() -> Regex {
    Regex::new(r"^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$")
        .expect("session id pattern is valid")
}

fn validate_session_id(id: &str) -> Result<(), SessionError> {
    if !session_id_pattern().is_match(id) {
        return Err(SessionError::new(
            SessionErrorCode::InvalidPayload,
            "Session id must be non-empty, contain only alphanumeric characters, '-', '_', and '.', and start and end with an alphanumeric character",
        ));
    }
    Ok(())
}

/// cwd → 会话目录名(对齐 TS `jsonlSessionDirectoryName`)。
pub fn jsonl_session_directory_name(cwd: &str) -> String {
    let stripped = cwd.trim_start_matches(['/', '\\']);
    let encoded: String = stripped
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect();
    format!("--{encoded}--")
}

async fn jsonl_sessions_root(options: &JsonlSessionRepoOptions) -> Result<String, SessionError> {
    file_result(
        options
            .fs
            .absolute_path(options.sessions_root.clone(), None)
            .await,
        &format!("Failed to resolve sessions root {}", options.sessions_root),
    )
}

async fn jsonl_session_directory(
    fs: &dyn FileSystem,
    sessions_root: &str,
    cwd: &str,
) -> Result<String, SessionError> {
    file_result(
        fs.join_path(
            vec![sessions_root.to_string(), jsonl_session_directory_name(cwd)],
            None,
        )
        .await,
        &format!("Failed to resolve sessions directory for {cwd}"),
    )
}

async fn jsonl_session_directories(
    options: &JsonlSessionRepoOptions,
    cwd: Option<&str>,
) -> Result<Vec<String>, SessionError> {
    let sessions_root = jsonl_sessions_root(options).await?;
    if let Some(cwd) = cwd {
        let resolved_cwd = file_result(
            options.fs.absolute_path(cwd.to_string(), None).await,
            &format!("Failed to resolve session cwd {cwd}"),
        )?;
        let directory =
            jsonl_session_directory(options.fs.as_ref(), &sessions_root, &resolved_cwd).await?;
        let exists = file_result(
            options.fs.exists(directory.clone(), None).await,
            &format!("Failed to check sessions directory {directory}"),
        )?;
        return Ok(if exists { vec![directory] } else { Vec::new() });
    }
    let exists = file_result(
        options.fs.exists(sessions_root.clone(), None).await,
        &format!("Failed to check sessions directory {sessions_root}"),
    )?;
    if !exists {
        return Ok(Vec::new());
    }
    let entries = file_result(
        options.fs.list_dir(sessions_root.clone(), None).await,
        &format!("Failed to list sessions directory {sessions_root}"),
    )?;
    Ok(entries
        .into_iter()
        .filter(|entry| entry.kind == FileKind::Directory || entry.kind == FileKind::Symlink)
        .map(|entry| entry.path)
        .collect())
}

/// 列出 JSONL 会话元数据(对齐 TS `listJsonlSessionMetadata`;按 modifiedAt 倒序)。
pub async fn list_jsonl_session_metadata(
    options: &JsonlSessionRepoOptions,
    query: &JsonlSessionListOptions,
) -> Result<Vec<JsonlSessionMetadata>, SessionError> {
    let mut metadata: Vec<JsonlSessionMetadata> = Vec::new();
    for directory in jsonl_session_directories(options, query.cwd.as_deref()).await? {
        let files = file_result(
            options.fs.list_dir(directory.clone(), None).await,
            &format!("Failed to list sessions directory {directory}"),
        )?
        .into_iter()
        .filter(|entry| entry.kind != FileKind::Directory && entry.name.ends_with(".jsonl"));
        for file in files {
            let lines = file_result(
                options
                    .fs
                    .read_text_lines(
                        file.path.clone(),
                        crate::agent::harness::types::ReadTextLinesOptions {
                            max_lines: Some(1),
                            abort_signal: None,
                        },
                    )
                    .await,
                &format!("Failed to read session header {}", file.path),
            )?;
            let Some(first_line) = lines.first() else {
                continue;
            };
            if first_line.is_empty() {
                continue;
            }
            let Ok(header) = parse_header(first_line) else {
                continue;
            };
            metadata.push(metadata_from_header(&header, &file.path, file.mtime_ms));
        }
    }
    metadata.sort_by(|left, right| {
        right
            .modified_at
            .partial_cmp(&left.modified_at)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(metadata)
}

/// 加载 JSONL 会话存储(对齐 TS `loadJsonlSessionStorage`)。
pub async fn load_jsonl_session_storage(
    options: &JsonlSessionRepoOptions,
    metadata: &JsonlSessionMetadata,
) -> Result<JsonlSessionStorage, SessionError> {
    let exists = file_result(
        options.fs.exists(metadata.path.clone(), None).await,
        &format!("Failed to check session {}", metadata.path),
    )?;
    if !exists {
        return Err(SessionError::new(
            SessionErrorCode::NotFound,
            format!("Session not found: {}", metadata.id),
        ));
    }
    let storage = JsonlSessionStorage::load(options.fs.clone(), &metadata.path).await?;
    let loaded_metadata = storage.metadata().await;
    if loaded_metadata.id != metadata.id {
        return Err(SessionError::new(
            SessionErrorCode::InvalidEntry,
            format!("Session id does not match header: {}", metadata.id),
        ));
    }
    Ok(storage)
}

/// `createdAt` → 会话文件名的时间段(`:`/`.` → `-`;对齐 TS `sessionFileName`)。
fn session_file_name(created_at: i64, id: &str) -> String {
    let timestamp = chrono::Utc
        .timestamp_millis_opt(created_at)
        .single()
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Millis, true))
        .unwrap_or_else(|| format!("{created_at}"));
    let timestamp = timestamp.replace([':', '.'], "-");
    format!("{timestamp}_{id}.jsonl")
}

/// JSONL 会话仓库(对齐 TS `JsonlSessionRepo`)。
pub struct JsonlSessionRepo {
    fs: Arc<dyn FileSystem>,
    sessions_root_input: String,
    active_create_destinations: Mutex<HashSet<String>>,
}

impl JsonlSessionRepo {
    pub fn new(options: JsonlSessionRepoOptions) -> Self {
        Self {
            fs: options.fs,
            sessions_root_input: options.sessions_root,
            active_create_destinations: Mutex::new(HashSet::new()),
        }
    }

    fn repo_options(&self) -> JsonlSessionRepoOptions {
        JsonlSessionRepoOptions {
            fs: self.fs.clone(),
            sessions_root: self.sessions_root_input.clone(),
        }
    }

    /// 创建会话(对齐 TS `create`)。
    pub async fn create(
        &self,
        options: JsonlSessionCreateOptions,
    ) -> Result<Session, SessionError> {
        let destination = self.resolve_create_destination(&options).await?;
        self.claim_create_destination(&destination, || async {
            let (header, path) = self.prepare_create(&destination, &options).await?;
            let storage = JsonlSessionStorage::create(self.fs.clone(), &path, header).await?;
            Ok(Session::new(Arc::new(storage)))
        })
        .await
    }

    /// 打开会话(对齐 TS `open`)。
    pub async fn open(&self, metadata: &JsonlSessionMetadata) -> Result<Session, SessionError> {
        let storage = self.load_storage(metadata).await?;
        Ok(Session::new(Arc::new(storage)))
    }

    /// 列出会话元数据(对齐 TS `list`)。
    pub async fn list(
        &self,
        options: &JsonlSessionListOptions,
    ) -> Result<Vec<JsonlSessionMetadata>, SessionError> {
        list_jsonl_session_metadata(&self.repo_options(), options).await
    }

    /// 删除会话文件(对齐 TS `delete`)。
    pub async fn delete(&self, metadata: &JsonlSessionMetadata) -> Result<(), SessionError> {
        file_result(
            self.fs
                .remove(
                    metadata.path.clone(),
                    crate::agent::harness::types::RemoveOptions {
                        recursive: None,
                        force: Some(true),
                        abort_signal: None,
                    },
                )
                .await,
            &format!("Failed to delete session {}", metadata.path),
        )
    }

    /// fork 会话(对齐 TS `fork`;parentSessionId 缺省取源会话)。
    pub async fn fork(
        &self,
        source: &JsonlSessionMetadata,
        options: &ForkOptions,
        create: &JsonlSessionCreateOptions,
    ) -> Result<Session, SessionError> {
        let source_storage = self.load_storage(source).await?;
        let create_options = JsonlSessionCreateOptions {
            parent_session_id: create
                .parent_session_id
                .clone()
                .or_else(|| Some(source.id.clone())),
            ..create.clone()
        };
        let destination = self.resolve_create_destination(&create_options).await?;
        self.claim_create_destination(&destination, || async {
            let (header, path) = self.prepare_create(&destination, &create_options).await?;
            let storage = source_storage.fork(&path, header, options).await?;
            Ok(Session::new(Arc::new(storage)))
        })
        .await
    }

    async fn load_storage(
        &self,
        metadata: &JsonlSessionMetadata,
    ) -> Result<JsonlSessionStorage, SessionError> {
        load_jsonl_session_storage(&self.repo_options(), metadata).await
    }

    async fn resolve_create_destination(
        &self,
        options: &JsonlSessionCreateOptions,
    ) -> Result<CreateDestination, SessionError> {
        let id = options.id.clone().unwrap_or_else(uuid_v7);
        validate_session_id(&id)?;
        let cwd = file_result(
            self.fs.absolute_path(options.cwd.clone(), None).await,
            &format!("Failed to resolve session cwd {}", options.cwd),
        )?;
        Ok(CreateDestination { id, cwd })
    }

    /// 同进程 create/fork 目的地互斥(对齐 TS `claimCreateDestination`)。
    async fn claim_create_destination<T, F>(
        &self,
        destination: &CreateDestination,
        operation: impl FnOnce() -> F,
    ) -> Result<T, SessionError>
    where
        F: std::future::Future<Output = Result<T, SessionError>>,
    {
        let key = format!("{}\u{0}{}", destination.cwd, destination.id);
        {
            let mut active = self
                .active_create_destinations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if active.contains(&key) {
                return Err(SessionError::new(
                    SessionErrorCode::AlreadyExists,
                    format!("Session already exists: {}", destination.id),
                ));
            }
            active.insert(key.clone());
        }
        let result = operation().await;
        {
            let mut active = self
                .active_create_destinations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            active.remove(&key);
        }
        result
    }

    async fn prepare_create(
        &self,
        destination: &CreateDestination,
        options: &JsonlSessionCreateOptions,
    ) -> Result<(JsonlV4Header, String), SessionError> {
        let CreateDestination { id, cwd } = destination;
        if self.session_id_exists(id, cwd).await? {
            return Err(SessionError::new(
                SessionErrorCode::AlreadyExists,
                format!("Session already exists: {id}"),
            ));
        }
        let created_at = now_ms();
        let session_directory = self.session_directory(cwd).await?;
        let path = file_result(
            self.fs
                .join_path(
                    vec![session_directory.clone(), session_file_name(created_at, id)],
                    None,
                )
                .await,
            &format!("Failed to resolve path for session {id}"),
        )?;
        if let Some(metadata) = &options.metadata {
            crate::agent::harness::session::session::assert_json_serializable(
                &serde_json::Value::Object(metadata.clone()),
            )?;
        }
        let mut header = JsonlV4Header::new(id.clone(), created_at, cwd.clone());
        header.parent_session_id = options.parent_session_id.clone();
        header.metadata = options.metadata.clone();
        file_result(
            self.fs
                .create_dir(
                    session_directory,
                    CreateDirOptions {
                        recursive: Some(true),
                        abort_signal: None,
                    },
                )
                .await,
            "Failed to create sessions directory",
        )?;
        Ok((header, path))
    }

    async fn session_id_exists(&self, id: &str, cwd: &str) -> Result<bool, SessionError> {
        let suffix = format!("_{id}.jsonl");
        let directory = self.session_directory(cwd).await?;
        let exists = file_result(
            self.fs.exists(directory.clone(), None).await,
            &format!("Failed to check sessions directory {directory}"),
        )?;
        if !exists {
            return Ok(false);
        }
        let files = file_result(
            self.fs.list_dir(directory, None).await,
            "Failed to list sessions directory",
        )?;
        Ok(files
            .iter()
            .any(|entry| entry.kind != FileKind::Directory && entry.name.ends_with(&suffix)))
    }

    async fn session_directory(&self, cwd: &str) -> Result<String, SessionError> {
        file_result(
            self.fs
                .join_path(
                    vec![self.root().await?, jsonl_session_directory_name(cwd)],
                    None,
                )
                .await,
            &format!("Failed to resolve sessions directory for {cwd}"),
        )
    }

    async fn root(&self) -> Result<String, SessionError> {
        file_result(
            self.fs
                .absolute_path(self.sessions_root_input.clone(), None)
                .await,
            &format!(
                "Failed to resolve sessions root {}",
                self.sessions_root_input
            ),
        )
    }
}

struct CreateDestination {
    id: String,
    cwd: String,
}
