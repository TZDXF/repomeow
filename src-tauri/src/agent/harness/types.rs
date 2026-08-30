//! harness 基础类型:对齐 `packages/agent/src/harness/types.ts`。
//!
//! TS `Result<TValue, TError>` 在 Rust 直接映射为 `std::result::Result`;
//! `ok`/`err`/`getOrThrow`/`getOrUndefined`/`toError` 保留同名辅助以对齐调用形状。
//! `FileError`/`ExecutionError`/`CompactionError`/`BranchSummaryError` 为带稳定
//! 错误码的结构化错误(thiserror 派生 Display/Error)。
//! `FileSystem`/`Shell`/`ExecutionEnv` 为对象安全的异步 trait(BoxFuture),
//! 具体异步后端见 [`crate::agent::harness::env::TokioEnv`]。

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::agent::llm::types::{CacheRetention, Transport};
use crate::agent::types::{
    AbortSignal, AgentToolResult, AgentToolUpdateCallback, PrepareArgumentsFn, ToolExecutionError,
    ToolExecutionMode,
};

// ---------------------------------------------------------------------------
// Result 基础件(harness/types.ts 顶部)
// ---------------------------------------------------------------------------

/// TS `Result<TValue, TError>` 的 Rust 对应:直接使用标准 [`Result`](std::result::Result)。
pub type Result<TValue, TError> = std::result::Result<TValue, TError>;

/// 创建成功 [`Result`](std::result::Result)(对齐 TS `ok()`)。
pub fn ok<TValue, TError>(value: TValue) -> Result<TValue, TError> {
    Ok(value)
}

/// 创建失败 [`Result`](std::result::Result)(对齐 TS `err()`)。
pub fn err<TValue, TError>(error: TError) -> Result<TValue, TError> {
    Err(error)
}

/// 返回成功值,否则 panic(对齐 TS `getOrThrow` 的 throw 语义;仅测试与适配边界使用)。
pub fn get_or_throw<TValue, TError: std::fmt::Debug>(result: Result<TValue, TError>) -> TValue {
    match result {
        Ok(value) => value,
        Err(error) => panic!("getOrThrow called on Err: {error:?}"),
    }
}

/// 返回成功值或 `None`(对齐 TS `getOrUndefined`)。
pub fn get_or_undefined<TValue, TError>(result: Result<TValue, TError>) -> Option<TValue> {
    result.ok()
}

/// 普通错误承载结构:对齐 TS 的裸 `new Error(message)`(无稳定错误码场景)。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct SimpleError {
    pub message: String,
}

impl SimpleError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// 将任意可展示值归一化为错误对象(对齐 TS `toError`)。
pub fn to_error(error: impl std::fmt::Display) -> SimpleError {
    SimpleError::new(error.to_string())
}

/// 工具/加载逻辑常用的抛错辅助:`Result` → `ToolExecutionError`。
pub fn throw_tool_error<E: std::fmt::Debug + std::fmt::Display>(error: E) -> ToolExecutionError {
    Box::new(SimpleError::new(error.to_string()))
}

// ---------------------------------------------------------------------------
// Skill / PromptTemplate / Resources
// ---------------------------------------------------------------------------

/// `SKILL.md` 或应用提供的技能(对齐 TS `Skill`)。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_model_invocation: Option<bool>,
}

/// 可格式化为 prompt 的模板(对齐 TS `PromptTemplate`)。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplate {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub content: String,
}

/// 显式调用方法与 system-prompt 回调可用的资源(对齐 TS `AgentHarnessResources`)。
#[derive(Clone, Debug, Default)]
pub struct AgentHarnessResources {
    pub prompt_templates: Option<Vec<PromptTemplate>>,
    pub skills: Option<Vec<Skill>>,
}

// ---------------------------------------------------------------------------
// AgentHarnessTool(带 context 的 execute)
// ---------------------------------------------------------------------------

/// harness 工具上下文标记 trait:内置工具需要 [`crate::agent::harness::tools::tool_context::ExecutionToolContext`]。
///
/// 用 `Any` 超特性支持向下转型(应用可携带任意上下文对象)。
pub trait ToolContext: std::any::Any + Send + Sync {}

/// harness 工具的执行函数(比 core 的 `ToolExecuteFn` 多一个按回合快照解析的 context)。
pub type HarnessToolExecuteFn = Arc<
    dyn Fn(
            String,
            Value,
            Option<AbortSignal>,
            Option<AgentToolUpdateCallback>,
            Arc<dyn ToolContext>,
        ) -> BoxFuture<'static, Result<AgentToolResult, ToolExecutionError>>
        + Send
        + Sync,
>;

/// 由 [`AgentHarness`](crate::agent::harness::agent_harness::AgentHarness) 执行、带应用上下文的工具定义
/// (对齐 TS `AgentHarnessTool`)。
#[derive(Clone)]
pub struct AgentHarnessTool {
    pub name: String,
    pub label: String,
    pub description: String,
    /// 参数 JSON Schema(与 core `AgentTool.parameters` 同形状)。
    pub parameters: Value,
    pub execution_mode: Option<ToolExecutionMode>,
    pub prepare_arguments: Option<PrepareArgumentsFn>,
    /// 携带当前回合 context 的执行入口。
    pub execute: HarnessToolExecuteFn,
}

impl std::fmt::Debug for AgentHarnessTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHarnessTool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("execution_mode", &self.execution_mode)
            .finish_non_exhaustive()
    }
}

/// 静态上下文或每回合零参解析器(对齐 TS `AgentHarnessToolContextSource`)。
#[derive(Clone)]
pub enum AgentHarnessToolContextSource {
    Static(Arc<dyn ToolContext>),
    Resolved(Arc<dyn Fn() -> BoxFuture<'static, Arc<dyn ToolContext>> + Send + Sync>),
}

/// 把 harness 工具绑定到上下文来源,产出 core `AgentTool`(供 `AgentContext.tools` 使用)。
pub fn bind_harness_tool(tool: AgentHarnessTool, source: AgentHarnessToolContextSource) -> crate::agent::types::AgentTool {
    let execute = tool.execute.clone();
    let bound: crate::agent::types::ToolExecuteFn = Arc::new(
        move |tool_call_id: String,
              params: Value,
              signal: Option<AbortSignal>,
              on_update: Option<AgentToolUpdateCallback>| {
            let execute = execute.clone();
            let source = source.clone();
            Box::pin(async move {
                let context: Arc<dyn ToolContext> = match &source {
                    AgentHarnessToolContextSource::Static(context) => context.clone(),
                    AgentHarnessToolContextSource::Resolved(resolve) => resolve().await,
                };
                execute(tool_call_id, params, signal, on_update, context).await
            })
        },
    );
    crate::agent::types::AgentTool {
        name: tool.name,
        label: tool.label,
        description: tool.description,
        parameters: tool.parameters,
        execution_mode: tool.execution_mode,
        prepare_arguments: tool.prepare_arguments,
        execute: bound,
    }
}

// ---------------------------------------------------------------------------
// 流选项(harness 自有的策展子集)
// ---------------------------------------------------------------------------

/// harness 持有、按回合快照的策展流选项(对齐 TS `AgentHarnessStreamOptions`)。
#[derive(Clone, Debug, Default)]
pub struct AgentHarnessStreamOptions {
    pub transport: Option<Transport>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub headers: Option<HashMap<String, String>>,
    pub metadata: Option<HashMap<String, Value>>,
    pub cache_retention: Option<CacheRetention>,
}

/// provider 钩子按请求返回的流选项补丁(对齐 TS `AgentHarnessStreamOptionsPatch`)。
///
/// `headers`/`metadata` 的键值为 `None` 表示删除该键;整体为 `None` 表示清空全部。
#[derive(Clone, Debug, Default)]
pub struct AgentHarnessStreamOptionsPatch {
    pub transport: Option<Transport>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub headers: Option<Option<HashMap<String, Option<String>>>>,
    pub metadata: Option<Option<HashMap<String, Option<Value>>>>,
    pub cache_retention: Option<CacheRetention>,
}

// ---------------------------------------------------------------------------
// 文件/执行错误与能力接口
// ---------------------------------------------------------------------------

/// [`FileSystem`] 寻址的文件系统对象类别。符号链接不自动跟随。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

impl std::fmt::Display for FileKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            FileKind::File => "file",
            FileKind::Directory => "directory",
            FileKind::Symlink => "symlink",
        };
        f.write_str(text)
    }
}

/// [`FileSystem`] 文件操作返回的稳定、后端无关错误码(对齐 TS `FileErrorCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileErrorCode {
    Aborted,
    NotFound,
    PermissionDenied,
    NotDirectory,
    IsDirectory,
    Invalid,
    NotSupported,
    Unknown,
}

impl std::fmt::Display for FileErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            FileErrorCode::Aborted => "aborted",
            FileErrorCode::NotFound => "not_found",
            FileErrorCode::PermissionDenied => "permission_denied",
            FileErrorCode::NotDirectory => "not_directory",
            FileErrorCode::IsDirectory => "is_directory",
            FileErrorCode::Invalid => "invalid",
            FileErrorCode::NotSupported => "not_supported",
            FileErrorCode::Unknown => "unknown",
        };
        f.write_str(text)
    }
}

/// [`FileSystem`] 文件操作返回的错误(对齐 TS `FileError`)。
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct FileError {
    /// 后端无关错误码。
    pub code: FileErrorCode,
    pub message: String,
    /// 关联的寻址路径(可用时携带)。
    pub path: Option<String>,
    /// 根因(Arc 包装以便 Clone;TS 为 Error.cause)。
    #[source]
    pub cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl FileError {
    pub fn new(code: FileErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
            cause: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_cause(
        mut self,
        cause: Arc<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        self.cause = Some(cause);
        self
    }

    /// [`crate::agent::types::ToolExecutionError`] 形状转换(工具闭包抛错用)。
    pub fn into_tool_error(self) -> ToolExecutionError {
        Box::new(self)
    }
}

/// [`ExecutionEnv::exec`] 返回的稳定错误码(对齐 TS `ExecutionErrorCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionErrorCode {
    Aborted,
    Timeout,
    ShellUnavailable,
    SpawnError,
    CallbackError,
    Unknown,
}

impl std::fmt::Display for ExecutionErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ExecutionErrorCode::Aborted => "aborted",
            ExecutionErrorCode::Timeout => "timeout",
            ExecutionErrorCode::ShellUnavailable => "shell_unavailable",
            ExecutionErrorCode::SpawnError => "spawn_error",
            ExecutionErrorCode::CallbackError => "callback_error",
            ExecutionErrorCode::Unknown => "unknown",
        };
        f.write_str(text)
    }
}

/// [`Shell::exec`] 返回的错误(对齐 TS `ExecutionError`)。
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct ExecutionError {
    pub code: ExecutionErrorCode,
    pub message: String,
    #[source]
    pub cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl ExecutionError {
    pub fn new(code: ExecutionErrorCode, message: impl Into<String>) -> Self {
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

/// compaction 辅助函数返回的稳定错误码(对齐 TS `CompactionErrorCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionErrorCode {
    Aborted,
    SummarizationFailed,
}

impl std::fmt::Display for CompactionErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            CompactionErrorCode::Aborted => "aborted",
            CompactionErrorCode::SummarizationFailed => "summarization_failed",
        };
        f.write_str(text)
    }
}

/// compaction 辅助函数返回的错误(对齐 TS `CompactionError`)。
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct CompactionError {
    pub code: CompactionErrorCode,
    pub message: String,
    #[source]
    pub cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl CompactionError {
    pub fn new(code: CompactionErrorCode, message: impl Into<String>) -> Self {
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

/// 分支摘要辅助函数返回的稳定错误码(对齐 TS `BranchSummaryErrorCode`)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchSummaryErrorCode {
    Aborted,
    SummarizationFailed,
}

impl std::fmt::Display for BranchSummaryErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            BranchSummaryErrorCode::Aborted => "aborted",
            BranchSummaryErrorCode::SummarizationFailed => "summarization_failed",
        };
        f.write_str(text)
    }
}

/// 分支摘要辅助函数返回的错误(对齐 TS `BranchSummaryError`)。
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct BranchSummaryError {
    pub code: BranchSummaryErrorCode,
    pub message: String,
    #[source]
    pub cause: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl BranchSummaryError {
    pub fn new(code: BranchSummaryErrorCode, message: impl Into<String>) -> Self {
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

/// [`FileSystem`] 中一个文件系统对象的元数据(对齐 TS `FileInfo`)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    /// [`FileInfo::path`] 的 basename。
    pub name: String,
    /// 寻址空间内的绝对归一化路径。符号链接不跟随。
    pub path: String,
    pub kind: FileKind,
    /// 字节大小。
    pub size: u64,
    /// Unix epoch 毫秒。
    pub mtime_ms: f64,
}

/// [`FileSystem::read_text_lines`] 选项。
#[derive(Clone, Debug, Default)]
pub struct ReadTextLinesOptions {
    pub max_lines: Option<usize>,
    pub abort_signal: Option<AbortSignal>,
}

/// [`FileSystem::create_dir`] 选项(默认 recursive=true、无 abort)。
#[derive(Clone, Debug, Default)]
pub struct CreateDirOptions {
    pub recursive: Option<bool>,
    pub abort_signal: Option<AbortSignal>,
}

/// [`FileSystem::remove`] 选项(默认 recursive=false、force=false、无 abort)。
#[derive(Clone, Debug, Default)]
pub struct RemoveOptions {
    pub recursive: Option<bool>,
    pub force: Option<bool>,
    pub abort_signal: Option<AbortSignal>,
}

/// [`FileSystem::create_temp_file`] 选项(默认 prefix=""、suffix=""、无 abort)。
#[derive(Clone, Debug, Default)]
pub struct CreateTempFileOptions {
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub abort_signal: Option<AbortSignal>,
}

/// 文件内容(文本或二进制)。
#[derive(Clone, Debug)]
pub enum FileContent {
    Text(String),
    Binary(Vec<u8>),
}

impl From<String> for FileContent {
    fn from(text: String) -> Self {
        FileContent::Text(text)
    }
}

impl From<&str> for FileContent {
    fn from(text: &str) -> Self {
        FileContent::Text(text.to_string())
    }
}

impl From<Vec<u8>> for FileContent {
    fn from(bytes: Vec<u8>) -> Self {
        FileContent::Binary(bytes)
    }
}

/// harness 使用的文件系统能力(对齐 TS `FileSystem`)。
///
/// 契约(与蓝本一致):操作方法绝不 panic/拒绝;所有失败(含后端意外失败)都
/// 编码进返回的 `Result`。路径可为绝对或相对 [`FileSystem::cwd`]。
pub trait FileSystem: Send + Sync {
    /// 相对路径的工作目录。
    fn cwd(&self) -> &str;

    /// 返回绝对寻址路径,不要求存在、不解析符号链接。
    fn absolute_path<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// 在文件系统命名空间内拼接路径段,不要求结果存在。
    fn join_path<'a>(
        &'a self,
        parts: Vec<String>,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// 读取 UTF-8 文本文件。
    fn read_text_file<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// 读取 UTF-8 文本行;`maxLines` 达到后应尽快停止读取。
    fn read_text_lines<'a>(
        &'a self,
        path: String,
        options: ReadTextLinesOptions,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>>;

    /// 读取二进制文件。
    fn read_binary_file<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>>;

    /// 创建或覆盖文件;支持时创建父目录。
    fn write_file<'a>(
        &'a self,
        path: String,
        content: FileContent,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<(), FileError>>;

    /// 创建或追加文件;支持时创建父目录。
    fn append_file<'a>(
        &'a self,
        path: String,
        content: FileContent,
    ) -> BoxFuture<'a, Result<(), FileError>>;

    /// 原子重命名文件,目标存在时替换;不跨文件系统复制。
    fn rename_file<'a>(
        &'a self,
        source_path: String,
        destination_path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<(), FileError>>;

    /// 返回寻址路径的元数据,不跟随符号链接。
    fn file_info<'a>(&'a self, path: String) -> BoxFuture<'a, Result<FileInfo, FileError>>;

    /// 列出目录直接子项,不跟随符号链接。
    fn list_dir<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>>;

    /// 返回已存在路径的规范路径,尽可能解析符号链接。
    fn canonical_path<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// 缺失路径返回 `Ok(false)`;权限等其他失败返回 `FileError`。
    fn exists<'a>(
        &'a self,
        path: String,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<bool, FileError>>;

    /// 创建目录;默认 recursive=true、无 abort。
    fn create_dir<'a>(&'a self, path: String, options: CreateDirOptions)
        -> BoxFuture<'a, Result<(), FileError>>;

    /// 删除文件或目录;默认 recursive=false、force=false、无 abort。
    fn remove<'a>(&'a self, path: String, options: RemoveOptions) -> BoxFuture<'a, Result<(), FileError>>;

    /// 创建临时目录并返回绝对路径;默认 prefix="tmp-"、无 abort。
    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<String>,
        abort_signal: Option<AbortSignal>,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// 创建临时文件并返回绝对路径;默认 prefix=""、suffix=""、无 abort。
    fn create_temp_file<'a>(
        &'a self,
        options: CreateTempFileOptions,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// 释放文件系统资源;尽力而为且绝不 panic。
    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()>;
}

/// [`Shell::exec`] 选项(对齐 TS `ShellExecOptions`)。
#[derive(Clone, Default)]
pub struct ShellExecOptions {
    /// 工作目录;相对路径按 `ExecutionEnv.cwd` 解析;缺省用 cwd。
    pub cwd: Option<String>,
    /// 命令环境变量;`inherit_env` 为 true 时覆盖继承的默认值。
    pub env: Option<HashMap<String, String>>,
    /// 是否继承执行环境默认变量;默认 true。
    pub inherit_env: Option<bool>,
    /// 超时(秒);超时返回 timeout 错误;默认无超时。
    pub timeout: Option<f64>,
    /// 中止信号。
    pub abort_signal: Option<AbortSignal>,
    /// stdout 块回调。
    pub on_stdout: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// stderr 块回调。
    pub on_stderr: Option<Arc<dyn Fn(String) + Send + Sync>>,
}

impl std::fmt::Debug for ShellExecOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellExecOptions")
            .field("cwd", &self.cwd)
            .field("env", &self.env.as_ref().map(|_| "<map>"))
            .field("inherit_env", &self.inherit_env)
            .field("timeout", &self.timeout)
            .field("has_abort", &self.abort_signal.is_some())
            .field("has_on_stdout", &self.on_stdout.is_some())
            .field("has_on_stderr", &self.on_stderr.is_some())
            .finish()
    }
}

/// [`Shell::exec`] 的成功输出。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// shell 执行能力(对齐 TS `Shell`)。
pub trait Shell: Send + Sync {
    /// 在 [`FileSystem::cwd`](除非 `options.cwd`)中执行 shell 命令。
    fn exec<'a>(
        &'a self,
        command: String,
        options: ShellExecOptions,
    ) -> BoxFuture<'a, Result<ExecOutcome, ExecutionError>>;

    /// 释放 shell 资源;尽力而为且绝不 panic。
    fn cleanup<'a>(&'a self) -> BoxFuture<'a, ()>;
}

/// 文件系统与进程执行环境(对齐 TS `ExecutionEnv extends FileSystem, Shell`)。
pub trait ExecutionEnv: FileSystem + Shell {}

impl<T: FileSystem + Shell> ExecutionEnv for T {}
