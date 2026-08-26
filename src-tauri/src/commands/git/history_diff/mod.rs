use super::*;

mod commit_changes;
mod commit_context;
mod history;
mod worktree_changes;

pub(crate) use history::run_git_log;
#[cfg(test)]
pub(crate) use commit_changes::{commit_file_diff_blocking, commit_files_blocking};
#[cfg(test)]
pub(crate) use commit_context::commit_context_blocking;
#[cfg(test)]
pub(crate) use history::{build_graph_revwalk, GraphDeco};
#[cfg(test)]
pub(crate) use worktree_changes::{worktree_file_diff_blocking, worktree_files_blocking};

/// 提交详情面板单文件 diff 的长度上限(超出截断,避免大文件撑爆 IPC)
const COMMIT_DIFF_MAX_CHARS: usize = 200_000;

/// 按 char 边界安全截断,返回 (文本, 是否截断)
fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    let end = text
        .char_indices()
        .nth(max)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    (text[..end].to_string(), true)
}

/// 单文件 diff 的展示选项(commit / worktree 单文件 diff 共用):
/// 全量上下文(完整文件内容,前端并排/逐行视图自行折叠未更改区间)+ 忽略空白差异。
/// context_lines 拉满:u32::MAX 会使 libgit2 的 hunk 边界计算溢出,
/// 产生 @@ -4,2- +4 @@ 畸形头且丢上下文;100k 已足够——行数超 10 万的文件
/// 体积必然超过 COMMIT_DIFF_MAX_CHARS 字符上限,会先被截断
fn apply_display_opts(opts: &mut DiffOptions, ignore_ws: Option<&str>) {
    opts.context_lines(100_000);
    // 忽略空白差异:eol=仅行尾 / change=空白数量变化 / all=全部空白(对应 git 的 -b / -w 语义)
    match ignore_ws {
        Some("eol") => {
            opts.ignore_whitespace_eol(true);
        }
        Some("change") => {
            opts.ignore_whitespace_change(true);
        }
        Some("all") => {
            opts.ignore_whitespace(true);
        }
        _ => {}
    }
}

#[tauri::command]
pub async fn git_commit_context(path: String) -> AppResult<GitCommitContext> {
    commit_context::git_commit_context(path).await
}

#[tauri::command]
pub async fn git_log(
    path: String,
    since: Option<String>,
    until: Option<String>,
    max_count: Option<u32>,
    author: Option<String>,
) -> AppResult<Vec<GitCommitInfo>> {
    history::git_log(path, since, until, max_count, author).await
}

#[tauri::command]
pub async fn git_graph_log(
    path: String,
    branches: Option<Vec<String>>,
    include_remote: Option<bool>,
    batch_size: Option<u32>,
    on_batch: Channel<GitGraphBatch>,
) -> AppResult<()> {
    history::git_graph_log(path, branches, include_remote, batch_size, on_batch).await
}

#[tauri::command]
pub async fn git_commit_files(path: String, hash: String) -> AppResult<Vec<GitCommitFile>> {
    commit_changes::git_commit_files(path, hash).await
}

#[tauri::command]
pub async fn git_commit_file_diff(
    path: String,
    hash: String,
    file_path: String,
    old_path: Option<String>,
    ignore_ws: Option<String>,
) -> AppResult<GitCommitFileDiff> {
    commit_changes::git_commit_file_diff(path, hash, file_path, old_path, ignore_ws).await
}

#[tauri::command]
pub async fn git_commit_file_blob(
    path: String,
    hash: String,
    file_path: String,
    parent: Option<bool>,
) -> AppResult<Option<String>> {
    commit_changes::git_commit_file_blob(path, hash, file_path, parent).await
}

#[tauri::command]
pub async fn git_worktree_files(path: String) -> AppResult<Vec<GitWorktreeFile>> {
    worktree_changes::git_worktree_files(path).await
}

#[tauri::command]
pub async fn git_worktree_file_diff(
    path: String,
    file_path: String,
    old_path: Option<String>,
    ignore_ws: Option<String>,
) -> AppResult<GitCommitFileDiff> {
    worktree_changes::git_worktree_file_diff(path, file_path, old_path, ignore_ws).await
}

/// 读取仓库当前 git 用户身份(日报"仅自己"过滤用)。
/// `git config user.name/email` 本身含全局配置回退;非仓库或未配置时返回空串而非报错
#[tauri::command]
pub async fn git_current_user(path: String) -> AppResult<GitUser> {
    run_blocking(move || run_git_current_user(&path)).await
}

pub(crate) fn run_git_current_user(path: &str) -> AppResult<GitUser> {
    // 仓库配置(libgit2 自动合并 local/global/system);非仓库回退全局配置,
    // 与 `git config user.name` 在仓库外仍读全局的行为一致
    let cfg = open_repo(path)
        .ok()
        .flatten()
        .and_then(|r| r.config().ok())
        .or_else(|| git2::Config::open_default().ok());
    let read = |key: &str| -> String {
        cfg.as_ref()
            .and_then(|c| c.get_string(key).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    Ok(GitUser {
        name: read("user.name"),
        email: read("user.email"),
    })
}
