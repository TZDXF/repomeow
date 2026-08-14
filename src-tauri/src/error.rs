use serde::Serialize;

/// 业务错误码,前端按 `as_str()` 查 `errors.<code>` i18n 文案。
/// 命名:小写蛇形,`domain_semantic`(如 `project_not_found` / `git_clone_failed`)。
/// 新增错误码时,前端 `src/i18n/locales/{zh-CN,en-US}.ts` 必须同步补 `errors.<code>`。
///
/// `message` 字段约定:仅存放不可本地化的技术上下文(路径、ID、HTTP 状态码、底层错误 detail),
/// 不放面向用户的完整句子;面向用户文案由 i18n 渲染,模板使用 `{context}` / `{path}` 等占位符
/// 由前端 `translateCommandError` 自动回填 `message`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    // 通用 / 内部错误兜底
    /// 数据库错误(来自 rusqlite)
    DbError,
    /// IO 错误(来自 std::io)
    IoError,
    /// 路径无效或目录不存在
    InvalidPath,

    // ── 项目 ──────────────────────────────────────────────────────────
    ProjectNotFound,
    ProjectNameRequired,
    ProjectPathConflict,
    MoveInvalidDirName,
    MoveSameLocation,
    MoveInsideSelf,
    MoveTargetExists,
    MoveRobocopyFailed,

    // ── Git ───────────────────────────────────────────────────────────
    GitCommandFailed,
    GitPushFailed,
    GitPullFailed,
    GitLogFailed,
    GitCloneUrlRequired,
    GitCloneInvalidTarget,
    GitCloneParentMissing,
    GitCloneTargetExists,
    GitCloneSpawnFailed,
    GitCloneCanceled,
    GitCloneFailed,
    GitClonePollFailed,
    GitBranchNameRequired,
    GitCommitMessageRequired,
    GitTaskFailed,
    GitNoiseFallback,
    GitLocalChangesConflict,
    GitUntrackedConflict,
    GitSshAuthFailed,
    GitHostKeyFailed,
    GitAuthFailed,
    GitRepoNotFound,
    GitNetworkDns,
    GitNetworkConnect,
    GitPushRejected,
    GitDiverged,
    GitNoTracking,
    /// 上游远程分支已被删除,pull/fetch 找不到对应 ref
    GitRemoteBranchGone,
    NotGitRepository,
    /// 分支已被其它 worktree 检出,不能重复检出
    GitBranchCheckedOut,
    /// worktree 含未提交修改或未跟踪文件,需强制删除
    GitWorktreeDirty,
    /// 同名分支已存在
    GitBranchExists,
    /// worktree 挂载远程分支时本地同名分支与其分叉,无法安全对齐
    GitBranchDiverged,
    /// 分支未完全合并,删除需强制(-D)
    GitBranchNotMerged,

    // ── 账号 ──────────────────────────────────────────────────────────
    AccountNotFound,
    AccountUnsupportedProvider,
    GitlabBaseUrlRequired,
    GitlabBaseUrlInvalidScheme,
    AccountTokenRequired,
    AccountTokenInvalid,
    GhCliNotFound,
    GhCliSpawnFailed,
    GhCliIncompleteCredentials,
    GhCliDetectFailed,
    GhCliCredentialsFailed,
    PlatformConnectionFailed,
    PlatformForbidden,
    PlatformNotFound,
    PlatformRequestFailed,
    PlatformRequestFailedWithDetail,
    UserInfoParseFailed,
    UserInfoMissingUsername,
    RepoListParseFailed,

    // ── Docker ────────────────────────────────────────────────────────
    DockerDirNotFound,
    DockerTaskFailed,
    DockerExecFailed,
    DockerActionFailed,
    DockerComposeParseFailed,
    DockerSaveFailed,
    DockerContainerNotCreated,
    DockerServiceImageMissing,
    DockerUnknownExportKind,
    DockerNoExportableImages,
    DockerNoExportableContainers,

    // ── 文件保存 ──────────────────────────────────────────────────────
    SavePathRequired,
    SaveContentTooLarge,
    SaveParentDirMissing,

    // ── 全文搜索 ──────────────────────────────────────────────────────
    /// 正则模式的搜索表达式非法(message 携带 regex 解析错误原文)
    SearchInvalidRegex,
    /// 文件包含/排除 glob 非法(message 携带 glob 解析错误原文)
    SearchInvalidGlob,

    // ── 隐藏项 / 标记 ─────────────────────────────────────────────────
    HiddenItemTypeUnknown,
    HiddenItemKeyRequired,
    PinTypeUnknown,
    PinKeyRequired,
    PinLabelCommandRequired,

    // ── 打开 ──────────────────────────────────────────────────────────
    /// 仅在非 Windows/macOS 平台下被构造(spawn_terminal / open_explorer 的兜底分支)
    #[allow(dead_code)]
    TerminalNotSupported,
    #[allow(dead_code)]
    FileManagerNotSupported,
    OpenMethodUnknown,
    CustomOpenCommandRequired,

    // ── 标签 ──────────────────────────────────────────────────────────
    TagNameRequired,
    TagColorInvalid,
    TagNameConflict,
    TagNotFound,

    // ── 自定义命令 ────────────────────────────────────────────────────
    CommandNameRequired,
    CommandContentRequired,
    CommandNameConflict,
    CommandNotFound,
    ScriptDirNotFound,

    // ── 报告 ──────────────────────────────────────────────────────────
    ReportInvalidYearMonth,
    ReportInvalidDate,
    ReportDateRangeInverted,
    ReportBatchWeeklySpanExceeded,
    ReportBatchDailySpanExceeded,
    ReportTaskFailed,

    // ── 定时任务 ──────────────────────────────────────────────────────
    ScheduleNotFound,
    SchedulerNoCommits,

    // ── AI ────────────────────────────────────────────────────────────
    AiNotConfigured,
    AiRequestFailed,
    AiResponseError,
    AiResponseParseFailed,
    AiEmptyResponse,

    // ── 工作日 ────────────────────────────────────────────────────────
    WorkdayHttpClientFailed,
    WorkdayFetchFailed,
    WorkdayResponseReadFailed,
    WorkdayParseFailed,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            // 通用
            Self::DbError => "db_error",
            Self::IoError => "io_error",
            Self::InvalidPath => "invalid_path",
            // 项目
            Self::ProjectNotFound => "project_not_found",
            Self::ProjectNameRequired => "project_name_required",
            Self::ProjectPathConflict => "project_path_conflict",
            Self::MoveInvalidDirName => "move_invalid_dir_name",
            Self::MoveSameLocation => "move_same_location",
            Self::MoveInsideSelf => "move_inside_self",
            Self::MoveTargetExists => "move_target_exists",
            Self::MoveRobocopyFailed => "move_robocopy_failed",
            // Git
            Self::GitCommandFailed => "git_command_failed",
            Self::GitPushFailed => "git_push_failed",
            Self::GitPullFailed => "git_pull_failed",
            Self::GitLogFailed => "git_log_failed",
            Self::GitCloneUrlRequired => "git_clone_url_required",
            Self::GitCloneInvalidTarget => "git_clone_invalid_target",
            Self::GitCloneParentMissing => "git_clone_parent_missing",
            Self::GitCloneTargetExists => "git_clone_target_exists",
            Self::GitCloneSpawnFailed => "git_clone_spawn_failed",
            Self::GitCloneCanceled => "git_clone_canceled",
            Self::GitCloneFailed => "git_clone_failed",
            Self::GitClonePollFailed => "git_clone_poll_failed",
            Self::GitBranchNameRequired => "git_branch_name_required",
            Self::GitCommitMessageRequired => "git_commit_message_required",
            Self::GitTaskFailed => "git_task_failed",
            Self::GitNoiseFallback => "git_noise_fallback",
            Self::GitLocalChangesConflict => "git_local_changes_conflict",
            Self::GitUntrackedConflict => "git_untracked_conflict",
            Self::GitSshAuthFailed => "git_ssh_auth_failed",
            Self::GitHostKeyFailed => "git_host_key_failed",
            Self::GitAuthFailed => "git_auth_failed",
            Self::GitRepoNotFound => "git_repo_not_found",
            Self::GitNetworkDns => "git_network_dns",
            Self::GitNetworkConnect => "git_network_connect",
            Self::GitPushRejected => "git_push_rejected",
            Self::GitDiverged => "git_diverged",
            Self::GitNoTracking => "git_no_tracking",
            Self::GitRemoteBranchGone => "git_remote_branch_gone",
            Self::NotGitRepository => "not_git_repository",
            Self::GitBranchCheckedOut => "git_branch_checked_out",
            Self::GitWorktreeDirty => "git_worktree_dirty",
            Self::GitBranchExists => "git_branch_exists",
            Self::GitBranchDiverged => "git_branch_diverged",
            Self::GitBranchNotMerged => "git_branch_not_merged",
            // 账号
            Self::AccountNotFound => "account_not_found",
            Self::AccountUnsupportedProvider => "account_unsupported_provider",
            Self::GitlabBaseUrlRequired => "gitlab_base_url_required",
            Self::GitlabBaseUrlInvalidScheme => "gitlab_base_url_invalid_scheme",
            Self::AccountTokenRequired => "account_token_required",
            Self::AccountTokenInvalid => "account_token_invalid",
            Self::GhCliNotFound => "gh_cli_not_found",
            Self::GhCliSpawnFailed => "gh_cli_spawn_failed",
            Self::GhCliIncompleteCredentials => "gh_cli_incomplete_credentials",
            Self::GhCliDetectFailed => "gh_cli_detect_failed",
            Self::GhCliCredentialsFailed => "gh_cli_credentials_failed",
            Self::PlatformConnectionFailed => "platform_connection_failed",
            Self::PlatformForbidden => "platform_forbidden",
            Self::PlatformNotFound => "platform_not_found",
            Self::PlatformRequestFailed => "platform_request_failed",
            Self::PlatformRequestFailedWithDetail => "platform_request_failed_with_detail",
            Self::UserInfoParseFailed => "user_info_parse_failed",
            Self::UserInfoMissingUsername => "user_info_missing_username",
            Self::RepoListParseFailed => "repo_list_parse_failed",
            // Docker
            Self::DockerDirNotFound => "docker_dir_not_found",
            Self::DockerTaskFailed => "docker_task_failed",
            Self::DockerExecFailed => "docker_exec_failed",
            Self::DockerActionFailed => "docker_action_failed",
            Self::DockerComposeParseFailed => "docker_compose_parse_failed",
            Self::DockerSaveFailed => "docker_save_failed",
            Self::DockerContainerNotCreated => "docker_container_not_created",
            Self::DockerServiceImageMissing => "docker_service_image_missing",
            Self::DockerUnknownExportKind => "docker_unknown_export_kind",
            Self::DockerNoExportableImages => "docker_no_exportable_images",
            Self::DockerNoExportableContainers => "docker_no_exportable_containers",
            // 文件保存
            Self::SavePathRequired => "save_path_required",
            Self::SaveContentTooLarge => "save_content_too_large",
            Self::SaveParentDirMissing => "save_parent_dir_missing",
            Self::SearchInvalidRegex => "search_invalid_regex",
            Self::SearchInvalidGlob => "search_invalid_glob",
            // 隐藏 / 标记
            Self::HiddenItemTypeUnknown => "hidden_item_type_unknown",
            Self::HiddenItemKeyRequired => "hidden_item_key_required",
            Self::PinTypeUnknown => "pin_type_unknown",
            Self::PinKeyRequired => "pin_key_required",
            Self::PinLabelCommandRequired => "pin_label_command_required",
            // 打开
            Self::TerminalNotSupported => "terminal_not_supported",
            Self::FileManagerNotSupported => "file_manager_not_supported",
            Self::OpenMethodUnknown => "open_method_unknown",
            Self::CustomOpenCommandRequired => "custom_open_command_required",
            // 标签
            Self::TagNameRequired => "tag_name_required",
            Self::TagColorInvalid => "tag_color_invalid",
            Self::TagNameConflict => "tag_name_conflict",
            Self::TagNotFound => "tag_not_found",
            // 自定义命令
            Self::CommandNameRequired => "command_name_required",
            Self::CommandContentRequired => "command_content_required",
            Self::CommandNameConflict => "command_name_conflict",
            Self::CommandNotFound => "command_not_found",
            Self::ScriptDirNotFound => "script_dir_not_found",
            // 报告
            Self::ReportInvalidYearMonth => "report_invalid_year_month",
            Self::ReportInvalidDate => "report_invalid_date",
            Self::ReportDateRangeInverted => "report_date_range_inverted",
            Self::ReportBatchWeeklySpanExceeded => "report_batch_weekly_span_exceeded",
            Self::ReportBatchDailySpanExceeded => "report_batch_daily_span_exceeded",
            Self::ReportTaskFailed => "report_task_failed",
            // 定时任务
            Self::ScheduleNotFound => "schedule_not_found",
            Self::SchedulerNoCommits => "scheduler_no_commits",
            // AI
            Self::AiNotConfigured => "ai_not_configured",
            Self::AiRequestFailed => "ai_request_failed",
            Self::AiResponseError => "ai_response_error",
            Self::AiResponseParseFailed => "ai_response_parse_failed",
            Self::AiEmptyResponse => "ai_empty_response",
            // 工作日
            Self::WorkdayHttpClientFailed => "workday_http_client_failed",
            Self::WorkdayFetchFailed => "workday_fetch_failed",
            Self::WorkdayResponseReadFailed => "workday_response_read_failed",
            Self::WorkdayParseFailed => "workday_parse_failed",
        }
    }
}

/// 应用错误。三类构成:
/// - `Db` / `Io`:内部低层错误,经 `#[from]` 自动转换,序列化时映射为 `db_error` / `io_error`
/// - `Coded`:面向用户、需前端 i18n 的错误,`message` 仅含技术上下文(路径、ID 等)
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// 携带错误码和可显示技术上下文的结构化错误
    #[error("code={code:?} message={message}")]
    Coded { code: ErrorCode, message: String },
}

impl AppError {
    /// 构造 Coded 错误:`message` 字段只放不可本地化的技术上下文(路径、ID、状态码等)
    pub fn coded(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::Coded {
            code,
            message: message.into(),
        }
    }

    /// 取出错误码(总是有值,因为 Db/Io 也映射到对应码)
    pub fn code(&self) -> &'static str {
        match self {
            Self::Db(_) => ErrorCode::DbError.as_str(),
            Self::Io(_) => ErrorCode::IoError.as_str(),
            Self::Coded { code, .. } => code.as_str(),
        }
    }

    /// 取出错误码枚举(用于测试断言和未来扩展)。当前未使用但保留 API
    #[allow(dead_code)]
    pub fn code_enum(&self) -> ErrorCode {
        match self {
            Self::Db(_) => ErrorCode::DbError,
            Self::Io(_) => ErrorCode::IoError,
            Self::Coded { code, .. } => *code,
        }
    }

    /// 测试断言用:是否为指定错误码
    #[cfg(test)]
    pub fn is_code(&self, expected: ErrorCode) -> bool {
        self.code_enum() == expected
    }
}

// Tauri 命令错误序列化为 `{"code": "...", "message": "..."}` 对象。
// 前端按 code 本地化;无法识别 code 时显示 message(技术上下文)。
// Coded 错误的 message 只放不可本地化的内容(路径/ID/底层错误),
// 面向用户的完整句子由前端 i18n 模板渲染。
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("code", &self.code())?;
        let message = match self {
            Self::Coded { message, .. } => message.clone(),
            Self::Db(e) => e.to_string(),
            Self::Io(e) => e.to_string(),
        };
        s.serialize_field("message", &message)?;
        s.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        // 防止重构时手抖改 snake_case 字符串(前端 i18n key 依赖此值)
        assert_eq!(ErrorCode::ProjectNotFound.as_str(), "project_not_found");
        assert_eq!(ErrorCode::GitCloneFailed.as_str(), "git_clone_failed");
        assert_eq!(
            ErrorCode::AccountTokenInvalid.as_str(),
            "account_token_invalid"
        );
        assert_eq!(ErrorCode::InvalidPath.as_str(), "invalid_path");
        assert_eq!(ErrorCode::AiNotConfigured.as_str(), "ai_not_configured");
    }

    #[test]
    fn coded_error_keeps_code_and_message() {
        let e = AppError::coded(ErrorCode::GitCloneFailed, "exit=128");
        assert!(e.is_code(ErrorCode::GitCloneFailed));
        assert_eq!(e.code(), "git_clone_failed");
    }

    #[test]
    fn db_error_maps_to_db_error_code() {
        let e: AppError = rusqlite::Error::InvalidQuery.into();
        assert_eq!(e.code(), "db_error");
        assert!(e.is_code(ErrorCode::DbError));
    }

    #[test]
    fn io_error_maps_to_io_error_code() {
        let e: AppError = std::io::Error::new(std::io::ErrorKind::NotFound, "x").into();
        assert_eq!(e.code(), "io_error");
        assert!(e.is_code(ErrorCode::IoError));
    }

    #[test]
    fn serialize_emits_code_and_message() {
        let raw = AppError::coded(ErrorCode::GitCloneFailed, "exit=128");
        let json = serde_json::to_value(&raw).unwrap();
        assert_eq!(json["code"], "git_clone_failed");
        assert_eq!(json["message"], "exit=128");
    }

    #[test]
    fn serialize_db_error_emits_db_code() {
        let raw: AppError = rusqlite::Error::InvalidQuery.into();
        let json = serde_json::to_value(&raw).unwrap();
        assert_eq!(json["code"], "db_error");
        assert!(json["message"].as_str().unwrap().len() > 0);
    }
}
