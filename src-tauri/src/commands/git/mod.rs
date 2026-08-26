use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, State};
use tokio::sync::{Notify, Semaphore};

use git2::{
    BranchType, Delta, DiffFindOptions, DiffOptions, Patch, Repository, Sort, Status as Git2Status,
    StatusOptions,
};

use crate::commands::account;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{
    GitBranchTrack, GitBranches, GitCommitContext, GitCommitFile, GitCommitFileDiff, GitCommitInfo,
    GitGraphBatch, GitGraphCommit, GitMergeResult, GitPullResult, GitRebaseResult, GitStatus,
    GitUntrackedFile, GitUser, GitWorktree, GitWorktreeFile,
};

mod fetch;
use fetch::*;
mod lifecycle;
pub use lifecycle::cleanup_on_exit;
use lifecycle::*;
/// 统一 Git 检查范围。all/project 用于已登记项目,path 用于临时 worktree。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitCheckScope {
    All,
    Project {
        #[serde(rename = "projectId")]
        project_id: i64,
    },
    Path {
        path: String,
    },
}

/// 所有 Git 状态来源最终都发布为同一种事件,消费者按变化标记订阅所需逻辑。
#[derive(Debug, Clone, Serialize)]
pub struct GitProjectChangedPayload {
    pub project_id: Option<i64>,
    pub name: Option<String>,
    pub path: String,
    pub status: GitStatus,
    pub head_sha: Option<String>,
    pub head_changed: bool,
    pub auto_pulled: bool,
    pub pulled_commits: i32,
    pub source: String,
    pub wiki_auto_update: bool,
}

mod process;
use process::*;
pub(crate) use process::{git_command, run_git};
// ── git2(libgit2)读操作层 ────────────────────────────────────────────────
// 所有只读查询(status/分支/log/diff 等)走 libgit2,避免每次查询创建 git 子进程
// (Windows 上进程创建约 10-30ms,批量状态/图谱等高频路径收益显著);
// 写操作与网络操作(fetch/pull/push/clone)仍走下方 CLI 辅助以继承用户凭证环境。

/// 打开仓库(向上查找父目录,与 `git -C <path>` 语义一致)。
/// 非 git 仓库返回 Ok(None),其他错误(权限/损坏等)透传
mod status_monitor;
pub use status_monitor::*;
mod refs;
pub use refs::*;
mod operations;
pub use operations::*;
mod worktree;
pub use worktree::*;
mod integrate;
pub use integrate::*;
mod history_diff;
pub use history_diff::*;
mod clone;
pub use clone::*;
#[cfg(test)]
mod tests;
