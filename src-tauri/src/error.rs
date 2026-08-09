use serde::Serialize;

/// 业务错误码(可选,前端 i18n 用),未知错误保留原中文 message
///
/// 命名:小写蛇形,域_语义,如 `project_not_found` / `invalid_path`。
/// 新增错误码时,前端 `src/i18n/locales/{zh-CN,en-US}.ts` 必须同时补 `errors.<code>` 文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// 项目不存在
    ProjectNotFound,
    /// 目录/路径无效或不存在
    InvalidPath,
    /// 定时任务不存在
    ScheduleNotFound,
    /// AI 服务尚未配置
    AiNotConfigured,
    /// 本地修改与远端冲突(merge/checkout 会覆盖)
    GitLocalChangesConflict,
    /// 未跟踪文件与远端冲突(merge/checkout 会覆盖)
    GitUntrackedConflict,
    /// SSH 密钥认证失败
    GitSshAuthFailed,
    /// SSH 主机密钥校验失败
    GitHostKeyFailed,
    /// 用户名密码/Token 认证失败
    GitAuthFailed,
    /// 远端仓库不存在或无权限
    GitRepoNotFound,
    /// 远端主机名无法解析
    GitNetworkDns,
    /// 无法连接远端服务器
    GitNetworkConnect,
    /// 推送被拒绝(远端有更新)
    GitPushRejected,
    /// 本地与远端分支已分叉
    GitDiverged,
    /// 当前分支未关联远端分支
    GitNoTracking,
    /// 当前目录不是 Git 仓库
    NotGitRepository,
    /// 平台账号 Token 无效或已过期
    AccountTokenInvalid,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::ProjectNotFound => "project_not_found",
            ErrorCode::InvalidPath => "invalid_path",
            ErrorCode::ScheduleNotFound => "schedule_not_found",
            ErrorCode::AiNotConfigured => "ai_not_configured",
            ErrorCode::GitLocalChangesConflict => "git_local_changes_conflict",
            ErrorCode::GitUntrackedConflict => "git_untracked_conflict",
            ErrorCode::GitSshAuthFailed => "git_ssh_auth_failed",
            ErrorCode::GitHostKeyFailed => "git_host_key_failed",
            ErrorCode::GitAuthFailed => "git_auth_failed",
            ErrorCode::GitRepoNotFound => "git_repo_not_found",
            ErrorCode::GitNetworkDns => "git_network_dns",
            ErrorCode::GitNetworkConnect => "git_network_connect",
            ErrorCode::GitPushRejected => "git_push_rejected",
            ErrorCode::GitDiverged => "git_diverged",
            ErrorCode::GitNoTracking => "git_no_tracking",
            ErrorCode::NotGitRepository => "not_git_repository",
            ErrorCode::AccountTokenInvalid => "account_token_invalid",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("记录不存在: {0}")]
    NotFound(String),
    #[error("冲突: {0}")]
    Conflict(String),
    #[error("无效输入: {0}")]
    Invalid(String),
    // Windows 上不会触发(仅非 Win/Mac 平台兜底使用)
    #[allow(dead_code)]
    #[error("外部命令失败: {0}")]
    External(String),
    /// 携带错误码和可显示消息的结构化错误
    #[error("{message}")]
    Coded { code: ErrorCode, message: String },
}

impl AppError {
    /// 构造"项目不存在"错误
    pub fn project_not_found(id: i64) -> AppError {
        AppError::Coded {
            code: ErrorCode::ProjectNotFound,
            message: format!("project {id}"),
        }
    }

    /// 构造"路径无效/目录不存在"错误
    pub fn invalid_path(path: &str) -> AppError {
        AppError::Coded {
            code: ErrorCode::InvalidPath,
            message: format!("目录不存在: {path}"),
        }
    }

    /// 构造"定时任务不存在"错误
    pub fn schedule_not_found() -> AppError {
        AppError::Coded {
            code: ErrorCode::ScheduleNotFound,
            message: "定时任务不存在".into(),
        }
    }

    /// 构造 AI 未配置错误
    pub fn ai_not_configured() -> AppError {
        AppError::Coded {
            code: ErrorCode::AiNotConfigured,
            message: "AI 未配置".into(),
        }
    }

    /// 取错误码(若有)
    pub fn code(&self) -> Option<&'static str> {
        match self {
            AppError::Coded { code, .. } => Some(code.as_str()),
            _ => None,
        }
    }

    /// 测试断言用:是否为指定错误码
    #[cfg(test)]
    pub fn is_code(&self, expected: ErrorCode) -> bool {
        matches!(self, AppError::Coded { code, .. } if *code == expected)
    }
}

// Tauri 命令错误序列化为 `{"code": "...", "message": "..."}` 对象。
// 前端按 code 本地化,无法识别 code 时显示 message。
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("code", &self.code().unwrap_or(""))?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
