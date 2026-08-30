use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{
    schemars, tool, tool_handler, tool_router, transport::stdio, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::git::{commit_blocking, run_git};
use crate::commands::wiki::wiki_dir_in;
use crate::error::AppError;
use crate::path_util::{clean_str, to_forward_slash_str};
use crate::APP_DATA_DIR_NAME;

const WIKI_DIR_NAME: &str = "wiki";
const WIKI_META_FILE: &str = "meta.json";
const DATA_DIR_ENV: &str = "REPOMEOW_DATA_DIR";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommitCodeInput {
    /// Git 仓库目录。可以使用绝对路径，提交范围始终以仓库根目录为准。
    pub directory: String,
    /// Git 提交信息，不能为空。
    pub message: String,
    /// 可选的仓库相对路径列表。省略时提交全部变更（含未跟踪文件）。
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitCodeOutput {
    pub directory: String,
    pub commit_hash: String,
    pub short_hash: String,
    pub branch: Option<String>,
    pub committed_files: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetWikiDirectoryInput {
    /// RepoMeow 中项目登记使用的目录。路径会按 RepoMeow 的规则归一化后定位 Wiki。
    pub project_directory: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiDirectoryOutput {
    pub project_directory: String,
    pub wiki_directory: String,
    pub meta_path: String,
    pub meta: Value,
}

#[derive(Debug)]
struct ToolFailure {
    code: String,
    message: String,
    detail: Option<String>,
}

impl ToolFailure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn from_app(message: impl Into<String>, error: AppError) -> Self {
        Self {
            code: error.code().to_string(),
            message: message.into(),
            detail: Some(error.to_string()),
        }
    }

    fn into_result(self) -> CallToolResult {
        CallToolResult::structured_error(json!({
            "code": self.code,
            "message": self.message,
            "detail": self.detail,
        }))
    }
}

#[derive(Debug, Clone)]
pub struct RepoMeowMcpServer {
    tool_router: ToolRouter<Self>,
}

impl Default for RepoMeowMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl RepoMeowMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "commit_code",
        description = "在指定 Git 仓库中创建代码提交。files 省略时提交全部变更（含未跟踪文件）；传入时仅提交指定的仓库相对路径。",
        annotations(
            title = "提交代码",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn commit_code(
        &self,
        Parameters(input): Parameters<CommitCodeInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let result = tokio::task::spawn_blocking(move || commit_code_impl(input)).await;
        Ok(match result {
            Ok(Ok(output)) => CallToolResult::structured(json!(output)),
            Ok(Err(error)) => error.into_result(),
            Err(error) => ToolFailure::new("git_task_failed", "代码提交任务执行失败")
                .with_detail(error.to_string())
                .into_result(),
        })
    }

    #[tool(
        name = "get_wiki_directory",
        description = "获取指定项目已经生成完成的 RepoMeow Wiki 目录和 meta.json 元数据。未生成 Wiki 时返回错误。",
        annotations(
            title = "获取 Wiki 目录",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_wiki_directory(
        &self,
        Parameters(input): Parameters<GetWikiDirectoryInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match get_wiki_directory_impl(input, None) {
            Ok(output) => CallToolResult::structured(json!(output)),
            Err(error) => error.into_result(),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RepoMeowMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("repomeow-mcp", env!("CARGO_PKG_VERSION"))
                    .with_title("RepoMeow MCP")
                    .with_description("RepoMeow 的代码提交与项目 Wiki 查询服务"),
            )
            .with_instructions(
                "仅提供两个工具：commit_code 用于创建 Git 提交；get_wiki_directory 用于读取已生成完成的 Wiki 目录和 meta.json。",
            )
    }
}

pub async fn serve_stdio() -> anyhow::Result<()> {
    let service = RepoMeowMcpServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

fn commit_code_impl(input: CommitCodeInput) -> Result<CommitCodeOutput, ToolFailure> {
    let directory = input.directory.trim();
    if directory.is_empty() || !Path::new(directory).is_dir() {
        return Err(ToolFailure::new(
            "invalid_directory",
            "代码提交目录不存在或不是文件夹",
        ));
    }
    if input.message.trim().is_empty() {
        return Err(ToolFailure::new(
            "git_commit_message_required",
            "Git 提交信息不能为空",
        ));
    }

    let root_output = run_git(directory, &["rev-parse", "--show-toplevel"])
        .map_err(|error| ToolFailure::from_app("无法定位 Git 仓库根目录", error))?;
    let root = String::from_utf8_lossy(&root_output.stdout)
        .trim()
        .to_string();
    if root.is_empty() {
        return Err(ToolFailure::new(
            "not_git_repository",
            "指定目录不是有效的 Git 工作区",
        ));
    }
    let root = clean_str(&root);
    let selected_files = normalize_commit_paths(input.files)?;

    let pathspecs = selected_files.as_ref().map(|paths| {
        paths
            .iter()
            .map(|path| format!(":(literal){path}"))
            .collect()
    });
    let status = commit_blocking(
        &root,
        input.message.trim(),
        selected_files.is_none(),
        pathspecs,
    );

    let status = status.map_err(|error| ToolFailure::from_app("代码提交失败", error))?;
    let hash = git_output(&root, &["rev-parse", "HEAD"], "读取提交哈希失败")?;
    let short_hash = git_output(
        &root,
        &["rev-parse", "--short", "HEAD"],
        "读取短提交哈希失败",
    )?;
    let committed_files = committed_files(&root)?;

    Ok(CommitCodeOutput {
        directory: root,
        commit_hash: hash,
        short_hash,
        branch: status.branch,
        committed_files,
    })
}

fn normalize_commit_paths(files: Option<Vec<String>>) -> Result<Option<Vec<String>>, ToolFailure> {
    let Some(files) = files else {
        return Ok(None);
    };
    if files.is_empty() {
        return Err(ToolFailure::new(
            "git_paths_required",
            "files 已提供时至少需要包含一个文件路径",
        ));
    }

    let mut normalized = Vec::with_capacity(files.len());
    for raw in files {
        let trimmed = raw.trim();
        let forward = to_forward_slash_str(trimmed);
        let looks_like_drive_path = forward.as_bytes().get(1) == Some(&b':');
        let invalid_component = Path::new(trimmed)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)));
        let invalid_forward_component = forward
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..");
        if trimmed.is_empty()
            || trimmed.contains('\0')
            || Path::new(trimmed).is_absolute()
            || forward.starts_with('/')
            || looks_like_drive_path
            || invalid_component
            || invalid_forward_component
        {
            return Err(ToolFailure::new(
                "invalid_file_path",
                format!("提交文件必须是仓库内的相对路径：{raw}"),
            ));
        }
        if !normalized.contains(&forward) {
            normalized.push(forward);
        }
    }
    Ok(Some(normalized))
}

fn git_output(root: &str, args: &[&str], message: &str) -> Result<String, ToolFailure> {
    let output = run_git(root, args).map_err(|error| ToolFailure::from_app(message, error))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn committed_files(root: &str) -> Result<Vec<String>, ToolFailure> {
    let output = run_git(
        root,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "HEAD",
        ],
    )
    .map_err(|error| ToolFailure::from_app("读取本次提交文件失败", error))?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| to_forward_slash_str(&String::from_utf8_lossy(path)))
        .collect())
}

fn get_wiki_directory_impl(
    input: GetWikiDirectoryInput,
    data_root: Option<&Path>,
) -> Result<WikiDirectoryOutput, ToolFailure> {
    let project_directory = clean_str(&input.project_directory);
    if project_directory.trim().is_empty() {
        return Err(ToolFailure::new(
            "invalid_project_directory",
            "项目目录不能为空",
        ));
    }

    let data_root = match data_root {
        Some(root) => root.to_path_buf(),
        None => repomeow_data_root()?,
    };
    let wiki_directory = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_directory);
    let meta_path = wiki_directory.join(WIKI_META_FILE);
    let raw = fs::read_to_string(&meta_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ToolFailure::new("wiki_not_generated", "该项目尚未生成 Wiki")
                .with_detail(meta_path.to_string_lossy())
        } else {
            ToolFailure::new("wiki_meta_read_failed", "读取 Wiki meta.json 失败")
                .with_detail(error.to_string())
        }
    })?;
    let meta: Value = serde_json::from_str(&raw).map_err(|error| {
        ToolFailure::new("wiki_meta_invalid", "Wiki meta.json 格式无效")
            .with_detail(error.to_string())
    })?;
    if meta.get("status").and_then(Value::as_str) != Some("completed") {
        return Err(
            ToolFailure::new("wiki_not_generated", "该项目的 Wiki 尚未生成完成")
                .with_detail(meta_path.to_string_lossy()),
        );
    }

    Ok(WikiDirectoryOutput {
        project_directory,
        wiki_directory: wiki_directory.to_string_lossy().into_owned(),
        meta_path: meta_path.to_string_lossy().into_owned(),
        meta,
    })
}

fn repomeow_data_root() -> Result<PathBuf, ToolFailure> {
    if let Some(path) = env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    home_dir()
        .map(|home| home.join(APP_DATA_DIR_NAME))
        .ok_or_else(|| ToolFailure::new("home_directory_unavailable", "无法确定当前用户主目录"))
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE").map(PathBuf::from).or_else(|| {
        let drive = env::var_os("HOMEDRIVE")?;
        let path = env::var_os("HOMEPATH")?;
        Some(PathBuf::from(drive).join(path))
    })
}

#[cfg(not(windows))]
fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "repomeow-mcp-{tag}-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos(),
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git(root: &Path, args: &[&str]) {
        let root = root.to_string_lossy();
        run_git(&root, args).unwrap();
    }

    #[test]
    fn commit_paths_only_accept_repository_relative_paths() {
        assert!(normalize_commit_paths(Some(vec!["src/main.rs".into()])).is_ok());
        assert!(normalize_commit_paths(Some(vec!["../secret".into()])).is_err());
        assert!(normalize_commit_paths(Some(vec!["C:/secret".into()])).is_err());
        assert!(normalize_commit_paths(Some(Vec::new())).is_err());
    }

    #[test]
    fn wiki_directory_returns_completed_meta() {
        let data_root = temp_dir("wiki-completed");
        let project = temp_dir("wiki-project");
        let project_path = project.to_string_lossy().into_owned();
        let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_path);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(WIKI_META_FILE),
            r#"{"status":"completed","version":1,"outline":[]}"#,
        )
        .unwrap();

        let output = get_wiki_directory_impl(
            GetWikiDirectoryInput {
                project_directory: project_path,
            },
            Some(&data_root),
        )
        .unwrap();
        assert_eq!(output.meta["status"], "completed");
        assert_eq!(PathBuf::from(output.meta_path), dir.join(WIKI_META_FILE));

        let _ = fs::remove_dir_all(data_root);
        let _ = fs::remove_dir_all(project);
    }

    #[test]
    fn wiki_directory_rejects_missing_or_incomplete_meta() {
        let data_root = temp_dir("wiki-missing");
        let project_path = "D:/projects/missing".to_string();
        let missing = get_wiki_directory_impl(
            GetWikiDirectoryInput {
                project_directory: project_path.clone(),
            },
            Some(&data_root),
        )
        .unwrap_err();
        assert_eq!(missing.code, "wiki_not_generated");

        let dir = wiki_dir_in(&data_root.join(WIKI_DIR_NAME), &project_path);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(WIKI_META_FILE), r#"{"status":"generating"}"#).unwrap();
        let incomplete = get_wiki_directory_impl(
            GetWikiDirectoryInput {
                project_directory: project_path,
            },
            Some(&data_root),
        )
        .unwrap_err();
        assert_eq!(incomplete.code, "wiki_not_generated");

        let _ = fs::remove_dir_all(data_root);
    }

    #[test]
    fn commit_code_can_commit_selected_files() {
        let root = temp_dir("commit-selected");
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.email", "mcp@example.com"]);
        git(&root, &["config", "user.name", "RepoMeow MCP"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();

        let output = commit_code_impl(CommitCodeInput {
            directory: root.to_string_lossy().into_owned(),
            message: "test: 仅提交 a".into(),
            files: Some(vec!["a.txt".into()]),
        })
        .unwrap();

        assert_eq!(output.branch.as_deref(), Some("main"));
        assert_eq!(output.committed_files, vec!["a.txt"]);
        let status = git_output(&output.directory, &["status", "--porcelain"], "status").unwrap();
        assert!(status.contains("?? b.txt"));

        let _ = fs::remove_dir_all(root);
    }
}
