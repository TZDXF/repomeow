use super::super::*;

#[test]
fn friendly_error_strips_ssh_noise_and_maps_local_changes() {
    let raw = "** WARNING: connection is not using a post-quantum key exchange algorithm.\n\
                   ** This session may be vulnerable to \"store now, decrypt later\" attacks.\n\
                   ** The server may need to be upgraded. See https://openssh.com/pq.html\n\
                   error: Your local changes to the following files would be overwritten by merge:\n\
                   \tpages/yudao/yudao-log/index.md\n\
                   Please commit your changes or stash them before you merge.\n\
                   error: The following untracked working tree files would be overwritten by merge:\n\
                   \tpages/yudao/yudao-log/log-2026.md\n\
                   Please move or remove them before you merge.\n\
                   Aborting";
    let err = friendly_git_error(raw);
    assert!(
        err.is_code(crate::error::ErrorCode::GitLocalChangesConflict),
        "实际输出: {err}"
    );
    let msg = err.to_string();
    assert!(!msg.contains("post-quantum"), "实际输出: {msg}");
    assert!(!msg.contains("Aborting"), "实际输出: {msg}");
}

#[test]
fn friendly_error_maps_untracked_overwritten() {
    let raw =
        "error: The following untracked working tree files would be overwritten by checkout:\n\
                   \tfoo.txt\n\
                   Please move or remove them before you switch branches.\n\
                   Aborting";
    let err = friendly_git_error(raw);
    assert!(
        err.is_code(crate::error::ErrorCode::GitUntrackedConflict),
        "实际输出: {err}"
    );
}

#[test]
fn friendly_error_maps_fetch_into_checked_out_branch() {
    // 拉取被 worktree 检出的分支:git 拒绝 fetch refspec 更新该引用,
    // 映射为专属错误码且 message 携带 worktree 路径供前端透出
    let raw = "From github.com:tzdxf/ruoyi-vue-pro\n\
                   fatal: refusing to fetch into branch 'refs/heads/zc-dev' checked out at 'D:/code/ruoyi-vue-pro/.worktrees/zc-dev'";
    let err = friendly_git_error(raw);
    assert!(
        err.is_code(crate::error::ErrorCode::GitFetchIntoCheckedOut),
        "实际输出: {err}"
    );
    let msg = err.to_string();
    assert!(msg.contains(".worktrees/zc-dev"), "实际输出: {msg}");
}

#[test]
fn friendly_error_keeps_no_upstream_branch_phrase() {
    // push_blocking 依赖该原文短语判断首推回退,映射不得覆盖
    let raw = "fatal: The current branch dev has no upstream branch.";
    let err = friendly_git_error(raw);
    assert_eq!(
        err.code(),
        "git_command_failed",
        "未识别错误应落到 git_command_failed: {err}"
    );
    assert!(
        err.to_string().contains("has no upstream branch"),
        "实际输出: {err}"
    );
}

#[test]
fn friendly_error_maps_common_cases() {
    use crate::error::ErrorCode;
    let cases: &[(&str, ErrorCode)] = &[
            (
                "git@github.com: Permission denied (publickey).",
                ErrorCode::GitSshAuthFailed,
            ),
            (
                "ssh: Could not resolve hostname github.com: Temporary failure in name resolution",
                ErrorCode::GitNetworkDns,
            ),
            (
                "error: failed to push some refs to 'origin'\nhint: Updates were rejected because the tip of your current branch is behind",
                ErrorCode::GitPushRejected,
            ),
            (
                "fatal: not a git repository (or any of the parent directories): .git",
                ErrorCode::NotGitRepository,
            ),
            (
                "remote: Repository not found.",
                ErrorCode::GitRepoNotFound,
            ),
            // 通用 git:"fatal: repository '<url>' not found"(URL 夹在中间,旧连续子串匹配漏判)
            (
                "fatal: repository 'http://192.168.1.3:12580/RD/ai-chat/graphrag-web-flask.git/' not found",
                ErrorCode::GitRepoNotFound,
            ),
            // GitLab:远端 404 时打印的 "project could not be found"
            (
                "remote: The project you were looking for could not be found.\n\
                 fatal: repository 'http://192.168.1.3:12580/RD/ai-chat/graphrag-web-flask.git/' not found",
                ErrorCode::GitRepoNotFound,
            ),
            (
                "fatal: You have divergent branches and need to specify how to reconcile them.",
                ErrorCode::GitDiverged,
            ),
        ];
    for (raw, expected) in cases {
        let err = friendly_git_error(raw);
        assert!(err.is_code(*expected), "输入 {raw:?} 实际输出: {err}");
    }
}

#[test]
fn friendly_error_all_noise_falls_back() {
    let err = friendly_git_error(
        "** WARNING: connection is not using a post-quantum key exchange algorithm.",
    );
    assert!(err.is_code(ErrorCode::GitNoiseFallback), "实际输出: {err}");
    assert_eq!(err.code(), "git_noise_fallback");
}

#[test]
fn friendly_error_maps_remote_branch_gone() {
    // 当前分支上游被删:git pull 的实际输出
    let pull_raw = "Your configuration specifies to merge with the ref 'refs/heads/feature'\n\
                        from the remote, but no such ref was fetched.";
    let err = friendly_git_error(pull_raw);
    assert!(
        err.is_code(ErrorCode::GitRemoteBranchGone),
        "实际输出: {err}"
    );
    // 指定分支拉取/抓取:git fetch origin feature:feature 的实际输出(版本间大小写不一)
    for raw in [
        "fatal: couldn't find remote ref feature",
        "fatal: Couldn't find remote ref feature",
    ] {
        let err = friendly_git_error(raw);
        assert!(
            err.is_code(ErrorCode::GitRemoteBranchGone),
            "输入 {raw:?} 实际输出: {err}"
        );
    }
}

