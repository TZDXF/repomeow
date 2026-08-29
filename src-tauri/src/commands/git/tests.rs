use super::*;
use std::fs;
use std::path::PathBuf;

#[test]
fn git_check_scope_deserializes_all_project_and_path() {
    assert!(matches!(
        serde_json::from_str::<GitCheckScope>(r#"{"kind":"all"}"#).unwrap(),
        GitCheckScope::All
    ));
    assert!(matches!(
        serde_json::from_str::<GitCheckScope>(r#"{"kind":"project","projectId":42}"#).unwrap(),
        GitCheckScope::Project { project_id: 42 }
    ));
    assert!(matches!(
        serde_json::from_str::<GitCheckScope>(r#"{"kind":"path","path":"D:/repo"}"#)
            .unwrap(),
        GitCheckScope::Path { path } if path == "D:/repo"
    ));
}

#[test]
fn fetch_registration_is_atomic_and_released_after_finish() {
    let path = format!("atomic-fetch-{}", crate::time_util::now_ts_nanos());
    assert!(try_begin_fetch(&path));
    assert!(!try_begin_fetch(&path));
    fetch_finished(&path, true);
    assert!(try_begin_fetch(&path));
    fetch_finished(&path, true);
}

#[test]
fn observe_head_only_reports_real_changes_after_initial_snapshot() {
    let path = format!("observe-head-{}", crate::time_util::now_ts_nanos());
    assert!(!observe_head(&path, Some("a".into()), false));
    assert!(!observe_head(&path, Some("a".into()), false));
    assert!(observe_head(&path, Some("b".into()), false));

    let forced = format!("observe-head-forced-{}", crate::time_util::now_ts_nanos());
    assert!(observe_head(&forced, Some("a".into()), true));
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-git-test-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &PathBuf, args: &[&str]) {
    let out = git_command(dir.to_str().unwrap())
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} 失败: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &PathBuf) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "test"]);
}

#[test]
fn commit_files_reports_status_and_line_counts() {
    let dir = temp_dir("commit-files");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    // M: a.txt 改两行;D: b.txt;A: 二进制 bin.dat
    fs::write(dir.join("a.txt"), "a1\na2\na3\n").unwrap();
    fs::remove_file(dir.join("b.txt")).unwrap();
    fs::write(dir.join("bin.dat"), [0u8, 159, 146, 150]).unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-m", "second"]);

    let files = commit_files_blocking(dir.to_str().unwrap(), "HEAD").unwrap();
    let by_path = |p: &str| files.iter().find(|f| f.path == p).cloned();
    let a = by_path("a.txt").expect("a.txt 应在清单中");
    assert_eq!(a.status, "M");
    assert_eq!(a.additions, Some(3));
    assert_eq!(a.deletions, Some(1));
    let b = by_path("b.txt").expect("b.txt 应在清单中");
    assert_eq!(b.status, "D");
    assert_eq!(b.deletions, Some(1));
    // 二进制:行数记 None
    let bin = by_path("bin.dat").expect("bin.dat 应在清单中");
    assert_eq!(bin.status, "A");
    assert_eq!(bin.additions, None);
    assert_eq!(bin.deletions, None);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_files_detects_rename_and_merge_returns_empty() {
    let dir = temp_dir("commit-files-rename");
    init_repo(&dir);
    fs::write(dir.join("old.txt"), "same content\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    git(&dir, &["mv", "old.txt", "new.txt"]);
    git(&dir, &["commit", "-m", "rename"]);

    let files = commit_files_blocking(dir.to_str().unwrap(), "HEAD").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, "R");
    assert_eq!(files[0].old_path.as_deref(), Some("old.txt"));
    assert_eq!(files[0].path, "new.txt");

    // 合并提交(多父)返回空,由前端提示
    git(&dir, &["checkout", "-b", "side", "HEAD~1"]);
    fs::write(dir.join("side.txt"), "s\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "side"]);
    git(&dir, &["checkout", "main"]);
    git(&dir, &["merge", "--no-ff", "-m", "merge", "side"]);
    let head = rev_parse(&dir, "HEAD");
    let files = commit_files_blocking(dir.to_str().unwrap(), &head).unwrap();
    assert!(files.is_empty(), "合并提交应返回空清单");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_file_diff_contains_hunks() {
    let dir = temp_dir("commit-diff");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    fs::write(dir.join("a.txt"), "hello world\n").unwrap();
    git(&dir, &["commit", "-am", "update"]);

    let d = commit_file_diff_blocking(dir.to_str().unwrap(), "HEAD", "a.txt", None, None).unwrap();
    assert!(d.diff.contains("@@"), "应含 hunk 头: {}", d.diff);
    assert!(d.diff.contains("+hello world"), "实际: {}", d.diff);
    assert!(!d.truncated);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_file_diff_add_only_keeps_full_context() {
    // 纯新增提交:hunk 头必须规整且包含全部上下文行(前端按完整文件内容渲染)
    let dir = temp_dir("commit-diff-addonly");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "l1\nl2\nl3\nl4\nl5\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    fs::write(dir.join("a.txt"), "l1\nl2\na1\na2\nl3\nl4\nl5\n").unwrap();
    git(&dir, &["commit", "-am", "add"]);

    let d = commit_file_diff_blocking(dir.to_str().unwrap(), "HEAD", "a.txt", None, None).unwrap();
    assert!(
        d.diff.contains("@@ -1,5 +1,7 @@"),
        "hunk 头应覆盖全文件: {}",
        d.diff
    );
    assert!(d.diff.contains("+a1"), "实际: {}", d.diff);
    assert!(d.diff.contains("\n l1\n"), "应保留上下文行: {}", d.diff);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remotes_list_names_and_urls() {
    let dir = temp_dir("remotes");
    init_repo(&dir);
    git(
        &dir,
        &["remote", "add", "origin", "git@github.com:user/repo.git"],
    );
    git(
        &dir,
        &["remote", "add", "upstream", "https://example.com/a/b.git"],
    );

    let remotes = list_remotes_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(remotes.len(), 2);
    assert_eq!(remotes[0].name, "origin");
    assert_eq!(remotes[0].url, "git@github.com:user/repo.git");
    assert_eq!(remotes[1].name, "upstream");
    assert_eq!(remotes[1].url, "https://example.com/a/b.git");

    // 非仓库 → 空列表
    let plain = temp_dir("remotes-plain");
    assert!(list_remotes_blocking(plain.to_str().unwrap())
        .unwrap()
        .is_empty());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&plain);
}

#[test]
fn non_repo_returns_is_repo_false() {
    let dir = temp_dir("plain");
    let st = status(dir.to_str().unwrap()).unwrap();
    assert!(!st.is_repo);
    let _ = fs::remove_dir_all(&dir);
}

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

#[test]
fn parses_working_tree_counts() {
    let dir = temp_dir("repo");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    // staged: 新文件 b; modified: a; untracked: c
    fs::write(dir.join("b.txt"), "b").unwrap();
    git(&dir, &["add", "b.txt"]);
    fs::write(dir.join("a.txt"), "changed").unwrap();
    fs::write(dir.join("c.txt"), "c").unwrap();

    let st = status(dir.to_str().unwrap()).unwrap();
    assert!(st.is_repo);
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.staged, 1);
    assert_eq!(st.modified, 1);
    assert_eq!(st.untracked, 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fetch_reports_remote_ahead() {
    // origin(bare) <- clone_a 推送; clone_b 作为被测项目
    let origin = temp_dir("origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone_a = temp_dir("clone-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);

    let clone_b = temp_dir("clone-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);

    // clone_a 再推一个提交,clone_b fetch 后 remote 领先 1
    fs::write(clone_a.join("a.txt"), "a2").unwrap();
    git(&clone_a, &["commit", "-am", "c2"]);
    git(&clone_a, &["push"]);

    let st = fetch_and_status(clone_b.to_str().unwrap()).unwrap();
    assert!(st.is_repo);
    assert_eq!(st.remote_ahead, 1);
    assert!(st.last_fetch_at.is_none());

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn commit_stages_all_and_cleans_worktree() {
    let dir = temp_dir("commit");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();

    let st = commit_blocking(dir.to_str().unwrap(), "init", true, None).unwrap();
    assert!(st.is_repo);
    assert_eq!(st.branch.as_deref(), Some("main"));
    assert_eq!(st.staged, 0);
    assert_eq!(st.modified, 0);
    assert_eq!(st.untracked, 0);

    // 空提交信息被拒绝
    assert!(commit_blocking(dir.to_str().unwrap(), "  ", true, None).is_err());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_untracked_is_opt_in() {
    let dir = temp_dir("commit-untracked");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("a.txt"), "changed").unwrap(); // 未暂存修改
    fs::write(dir.join("b.txt"), "b").unwrap(); // 未跟踪

    // 不勾选:未暂存修改照常提交,未跟踪文件保留
    let st = commit_blocking(dir.to_str().unwrap(), "tracked only", false, None).unwrap();
    assert_eq!(st.staged, 0);
    assert_eq!(st.modified, 0);
    assert_eq!(st.untracked, 1);

    // 勾选:未跟踪文件一并提交,工作区干净
    let st = commit_blocking(dir.to_str().unwrap(), "with untracked", true, None).unwrap();
    assert_eq!(st.staged, 0);
    assert_eq!(st.modified, 0);
    assert_eq!(st.untracked, 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_paths_partial_selection() {
    let dir = temp_dir("commit-paths");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    fs::write(dir.join("b.txt"), "b").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("a.txt"), "changed").unwrap(); // 未暂存修改
    fs::write(dir.join("b.txt"), "changed").unwrap(); // 未暂存修改
    fs::write(dir.join("c.txt"), "c").unwrap(); // 未跟踪

    // 空路径清单被拒绝
    assert!(commit_blocking(dir.to_str().unwrap(), "none", true, Some(Vec::new())).is_err());

    // 只提交 a.txt 与未跟踪的 c.txt:b.txt 修改保留在工作区
    let st = commit_blocking(
        dir.to_str().unwrap(),
        "partial",
        true,
        Some(vec!["a.txt".into(), "c.txt".into()]),
    )
    .unwrap();
    assert_eq!(st.staged, 0);
    assert_eq!(st.modified, 1);
    assert_eq!(st.untracked, 0);
    // 提交内容只含选中的两个文件
    let out = run_git(
        dir.to_str().unwrap(),
        &["show", "--name-status", "--format=", "HEAD"],
    )
    .unwrap();
    let names = String::from_utf8_lossy(&out.stdout);
    assert!(names.contains("M\ta.txt"));
    assert!(names.contains("A\tc.txt"));
    assert!(!names.contains("b.txt"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn worktree_files_cover_modified_staged_and_untracked() {
    let dir = temp_dir("worktree-files");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("a.txt"), "changed").unwrap(); // 未暂存修改
    fs::write(dir.join("b.txt"), "b").unwrap(); // 未跟踪
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/c.txt"), "c").unwrap(); // 未跟踪(嵌套目录)

    let files = worktree_files_blocking(dir.to_str().unwrap()).unwrap();
    let by_path: HashMap<_, _> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    assert_eq!(by_path.len(), 3, "文件清单: {files:?}");
    assert_eq!(by_path["a.txt"].status, "M");
    assert!(!by_path["a.txt"].untracked);
    assert!(by_path["b.txt"].untracked);
    assert!(by_path["sub/c.txt"].untracked);
    assert!(by_path["b.txt"].additions.unwrap_or(0) > 0);

    // 未跟踪文件的单文件 diff 是全新增补丁
    let d = worktree_file_diff_blocking(dir.to_str().unwrap(), "b.txt", None, None).unwrap();
    assert!(d.diff.contains("+b"), "未跟踪文件 diff: {}", d.diff);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn worktree_files_include_untracked_on_unborn_head() {
    // 新初始化仓库(尚无提交):未跟踪文件必须出现在提交预览里,
    // 否则 GitInitDialog 初始化后的首次提交对话框清单为空
    let dir = temp_dir("worktree-files-unborn");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap(); // 未跟踪
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("sub/b.txt"), "b").unwrap(); // 未跟踪(嵌套目录)

    let files = worktree_files_blocking(dir.to_str().unwrap()).unwrap();
    let by_path: HashMap<_, _> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    assert_eq!(by_path.len(), 2, "文件清单: {files:?}");
    assert_eq!(by_path["a.txt"].status, "A");
    assert!(by_path["a.txt"].untracked);
    assert!(by_path["sub/b.txt"].untracked);

    // 暂存后(仍未提交)同样可见:暂存的不再是 untracked 标记
    git(&dir, &["add", "a.txt"]);
    let files = worktree_files_blocking(dir.to_str().unwrap()).unwrap();
    let by_path: HashMap<_, _> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    assert_eq!(by_path.len(), 2, "文件清单: {files:?}");
    assert!(!by_path["a.txt"].untracked);
    assert!(by_path["sub/b.txt"].untracked);

    // 单文件 diff 同样可用
    let d = worktree_file_diff_blocking(dir.to_str().unwrap(), "a.txt", None, None).unwrap();
    assert!(d.diff.contains("+a"), "未出生 HEAD 单文件 diff: {}", d.diff);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branches_checkout_and_create() {
    let dir = temp_dir("branch");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(branches.local, vec!["main".to_string()]);
    assert!(branches.remote.is_empty());

    // 新建并切换
    let st = checkout_blocking(dir.to_str().unwrap(), "feature", true, false, None).unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(
        branches.local,
        vec!["feature".to_string(), "main".to_string()]
    );

    // 切回 main
    let st = checkout_blocking(dir.to_str().unwrap(), "main", false, false, None).unwrap();
    assert_eq!(st.branch.as_deref(), Some("main"));

    // 空分支名 / 不存在的分支
    assert!(checkout_blocking(dir.to_str().unwrap(), " ", false, false, None).is_err());
    assert!(checkout_blocking(dir.to_str().unwrap(), "nope", false, false, None).is_err());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branches_keep_log_style_names_when_remote_shares_branch_name() {
    // 分支 zc 与 remote zc 同名时(refs/remotes/zc/HEAD 存在),
    // %(refname:short) 为消歧输出 "heads/zc",而 git log %D 装饰仍显示 "zc";
    // 分支列表必须与 %D 一致,否则图谱侧栏点分支定位顶端提交失败
    let dir = temp_dir("ambiguous-remote");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);
    git(&dir, &["branch", "zc"]);
    let head = git_command(dir.to_str().unwrap())
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap()
        .stdout;
    let head = String::from_utf8_lossy(&head).trim().to_string();
    git(&dir, &["update-ref", "refs/remotes/zc/HEAD", &head]);
    git(&dir, &["update-ref", "refs/remotes/zc/zc", &head]);

    // 前提校验:git 的 short 命名在此场景下确实会消歧成 heads/zc
    let short = git_command(dir.to_str().unwrap())
        .args(["branch", "--format=%(refname:short)"])
        .output()
        .unwrap()
        .stdout;
    assert!(String::from_utf8_lossy(&short).contains("heads/zc"));

    let branches = list_branches_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(branches.local, vec!["main".to_string(), "zc".to_string()]);
    assert!(branches.remote.contains(&"zc/zc".to_string()));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn checkout_remote_creates_tracking_branch() {
    let origin = temp_dir("track-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    // clone_a:推 main 和 feature 两个分支到远端
    let clone_a = temp_dir("track-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);
    git(&clone_a, &["checkout", "-b", "feature"]);
    fs::write(clone_a.join("b.txt"), "b").unwrap();
    git(&clone_a, &["add", "b.txt"]);
    git(&clone_a, &["commit", "-m", "c2"]);
    git(&clone_a, &["push", "-u", "origin", "feature"]);

    let clone_b = temp_dir("track-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);

    // 远程分支列出 feature/main,不含 origin/HEAD 符号引用
    let branches = list_branches_blocking(clone_b.to_str().unwrap()).unwrap();
    assert_eq!(branches.local, vec!["main".to_string()]);
    assert_eq!(
        branches.remote,
        vec!["origin/feature".to_string(), "origin/main".to_string()]
    );

    // 检出远程分支:本地无同名分支 → 创建跟踪分支
    let st = checkout_blocking(
        clone_b.to_str().unwrap(),
        "origin/feature",
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    // 本地已有同名分支 → 直接切换(幂等,不报错)
    let st = checkout_blocking(
        clone_b.to_str().unwrap(),
        "origin/feature",
        false,
        true,
        None,
    )
    .unwrap();
    assert_eq!(st.branch.as_deref(), Some("feature"));

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn push_sets_upstream_when_missing() {
    let origin = temp_dir("push-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone = temp_dir("push-clone");
    git(&clone, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    git(&clone, &["config", "user.name", "test"]);
    fs::write(clone.join("a.txt"), "a").unwrap();
    git(&clone, &["add", "a.txt"]);
    git(&clone, &["commit", "-m", "c1"]);

    // 首次 push 无 upstream → 自动回退 `git push -u origin HEAD`
    let st = push_blocking(clone.to_str().unwrap()).unwrap();
    assert!(st.is_repo);

    // 已建立 upstream 后走普通 push 路径
    fs::write(clone.join("a.txt"), "a2").unwrap();
    git(&clone, &["commit", "-am", "c2"]);
    push_blocking(clone.to_str().unwrap()).unwrap();

    let out = git_command(origin.to_str().unwrap())
        .args(["rev-list", "--count", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2");

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone);
}

#[test]
fn push_first_time_uses_non_origin_remote() {
    let origin = temp_dir("push-nonorigin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone = temp_dir("push-nonorigin-clone");
    git(&clone, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    git(&clone, &["config", "user.name", "test"]);
    // 远端不叫 origin(如 "github")时,首推回退也应成功
    git(&clone, &["remote", "rename", "origin", "github"]);
    fs::write(clone.join("a.txt"), "a").unwrap();
    git(&clone, &["add", "a.txt"]);
    git(&clone, &["commit", "-m", "c1"]);

    let st = push_blocking(clone.to_str().unwrap()).unwrap();
    assert!(st.is_repo);

    // upstream 应指向 github/main
    let out = git_command(clone.to_str().unwrap())
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "github/main");

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone);
}

#[test]
fn split_remote_branch_parses_remote_and_branch() {
    assert_eq!(
        split_remote_branch("origin/feature/x"),
        Some(("origin".to_string(), "feature/x".to_string()))
    );
    assert_eq!(
        split_remote_branch("github/main"),
        Some(("github".to_string(), "main".to_string()))
    );
    assert_eq!(split_remote_branch("main"), None);
    assert_eq!(split_remote_branch("/main"), None);
    assert_eq!(split_remote_branch("origin/"), None);
}

fn clone_with_config(tag: &str, origin: &PathBuf) -> PathBuf {
    let dir = temp_dir(tag);
    git(&dir, &["clone", origin.to_str().unwrap(), "."]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "test"]);
    dir
}

fn rev_parse(dir: &PathBuf, rev: &str) -> String {
    let out = git_command(dir.to_str().unwrap())
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    assert!(out.status.success(), "rev-parse {rev} 失败");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// origin(bare) + clone_a:首推 main,并创建推送 feature 分支(clone_a 停留在 feature)
fn setup_origin_with_feature(tag: &str) -> (PathBuf, PathBuf) {
    let origin = temp_dir(&format!("{tag}-origin"));
    git(&origin, &["init", "--bare", "-b", "main"]);
    let clone_a = clone_with_config(&format!("{tag}-a"), &origin);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);
    git(&clone_a, &["checkout", "-b", "feature"]);
    fs::write(clone_a.join("f.txt"), "f1").unwrap();
    git(&clone_a, &["add", "f.txt"]);
    git(&clone_a, &["commit", "-m", "f1"]);
    git(&clone_a, &["push", "-u", "origin", "feature"]);
    (origin, clone_a)
}

#[test]
fn pull_branch_fast_forwards_non_current_branch() {
    let (origin, clone_a) = setup_origin_with_feature("pullbr-ff");
    let clone_b = clone_with_config("pullbr-ff-b", &origin);
    git(
        &clone_b,
        &["branch", "--track", "feature", "origin/feature"],
    );

    // clone_a 推进 feature 并推送,clone_b 的 feature 落后
    fs::write(clone_a.join("f.txt"), "f2").unwrap();
    git(&clone_a, &["commit", "-am", "f2"]);
    git(&clone_a, &["push"]);

    let result = pull_branch_blocking(clone_b.to_str().unwrap(), "feature").unwrap();
    assert!(result.conflicts.is_empty());
    // 工作区仍停留在 main,本地 feature 已快进到 origin/feature
    assert_eq!(result.status.branch.as_deref(), Some("main"));
    assert_eq!(
        rev_parse(&clone_b, "feature"),
        rev_parse(&clone_b, "origin/feature")
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn pull_branch_diverged_returns_error() {
    let (origin, clone_a) = setup_origin_with_feature("pullbr-div");
    let clone_b = clone_with_config("pullbr-div-b", &origin);
    // clone_b 在 feature 上产生本地提交后切回 main
    git(&clone_b, &["checkout", "feature"]);
    fs::write(clone_b.join("b.txt"), "b").unwrap();
    git(&clone_b, &["add", "b.txt"]);
    git(&clone_b, &["commit", "-m", "b1"]);
    git(&clone_b, &["checkout", "main"]);
    // clone_a 推进 feature,形成分叉
    fs::write(clone_a.join("f.txt"), "f2").unwrap();
    git(&clone_a, &["commit", "-am", "f2"]);
    git(&clone_a, &["push"]);

    assert!(pull_branch_blocking(clone_b.to_str().unwrap(), "feature").is_err());
    // 失败后本地 feature 未被改写
    assert_ne!(
        rev_parse(&clone_b, "feature"),
        rev_parse(&clone_b, "origin/feature")
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn pull_branch_remote_deleted_returns_gone_error() {
    let (origin, clone_a) = setup_origin_with_feature("pullbr-gone");
    let clone_b = clone_with_config("pullbr-gone-b", &origin);
    git(
        &clone_b,
        &["branch", "--track", "feature", "origin/feature"],
    );
    // 远端删除 feature
    git(&clone_a, &["push", "origin", "--delete", "feature"]);

    let err = pull_branch_blocking(clone_b.to_str().unwrap(), "feature").unwrap_err();
    assert!(
        err.is_code(crate::error::ErrorCode::GitRemoteBranchGone),
        "实际输出: {err}"
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn pull_current_branch_remote_deleted_returns_gone_error() {
    let (origin, clone_a) = setup_origin_with_feature("pull-gone");
    let clone_b = clone_with_config("pull-gone-b", &origin);
    git(&clone_b, &["checkout", "feature"]);
    // 远端删除 feature(不带 --prune,本地仍保留 origin/feature 引用)
    git(&clone_a, &["push", "origin", "--delete", "feature"]);

    let err = pull_blocking(clone_b.to_str().unwrap()).unwrap_err();
    assert!(
        err.is_code(crate::error::ErrorCode::GitRemoteBranchGone),
        "实际输出: {err}"
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn push_branch_pushes_to_upstream() {
    let (origin, clone_a) = setup_origin_with_feature("pushbr-up");
    let clone_b = clone_with_config("pushbr-up-b", &origin);
    git(&clone_b, &["checkout", "feature"]);
    fs::write(clone_b.join("b.txt"), "b").unwrap();
    git(&clone_b, &["add", "b.txt"]);
    git(&clone_b, &["commit", "-m", "b1"]);
    git(&clone_b, &["checkout", "main"]);

    push_branch_blocking(clone_b.to_str().unwrap(), "feature").unwrap();
    assert_eq!(
        rev_parse(&clone_b, "feature"),
        rev_parse(&clone_b, "origin/feature")
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn push_branch_without_upstream_sets_tracking() {
    let (origin, clone_a) = setup_origin_with_feature("pushbr-new");
    let clone_b = clone_with_config("pushbr-new-b", &origin);
    git(&clone_b, &["branch", "topic"]);

    push_branch_blocking(clone_b.to_str().unwrap(), "topic").unwrap();
    let out = git_command(clone_b.to_str().unwrap())
        .args(["rev-parse", "--abbrev-ref", "topic@{upstream}"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "origin/topic");
    assert_eq!(
        rev_parse(&clone_b, "topic"),
        rev_parse(&clone_b, "origin/topic")
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn branch_delete_merged_branch() {
    let dir = temp_dir("brdel-merged");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "c1"]);
    // topic 基于 main,无额外提交:视为已合并可安全删除
    git(&dir, &["branch", "topic"]);

    branch_delete_blocking(dir.to_str().unwrap(), "topic", false).unwrap();
    let out = git_command(dir.to_str().unwrap())
        .args(["branch", "--list", "topic"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branch_delete_unmerged_requires_force() {
    let dir = temp_dir("brdel-unmerged");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "c1"]);
    // topic 有未合并进 main 的提交
    git(&dir, &["checkout", "-b", "topic"]);
    fs::write(dir.join("t.txt"), "t").unwrap();
    git(&dir, &["add", "t.txt"]);
    git(&dir, &["commit", "-m", "t1"]);
    git(&dir, &["checkout", "main"]);

    let err = branch_delete_blocking(dir.to_str().unwrap(), "topic", false).unwrap_err();
    assert!(err.is_code(ErrorCode::GitBranchNotMerged));
    // 强删成功
    branch_delete_blocking(dir.to_str().unwrap(), "topic", true).unwrap();
    let out = git_command(dir.to_str().unwrap())
        .args(["branch", "--list", "topic"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn branch_delete_rejects_current_and_empty() {
    let dir = temp_dir("brdel-current");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "c1"]);

    // 空分支名
    let err = branch_delete_blocking(dir.to_str().unwrap(), "  ", false).unwrap_err();
    assert!(err.is_code(ErrorCode::GitBranchNameRequired));
    // 当前检出分支不可删除(git 拒绝),且分支仍在(--list 输出带 * 前缀)
    assert!(branch_delete_blocking(dir.to_str().unwrap(), "main", true).is_err());
    let out = git_command(dir.to_str().unwrap())
        .args(["branch", "--list", "main"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "* main");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remote_branch_delete_removes_remote_ref() {
    let (origin, clone_a) = setup_origin_with_feature("rdel");
    let clone_b = clone_with_config("rdel-b", &origin);

    // 删除 origin/feature(短名含多级目录时同样按首个 '/' 拆分)
    remote_branch_delete_blocking(clone_b.to_str().unwrap(), "origin/feature").unwrap();
    let out = git_command(clone_b.to_str().unwrap())
        .args(["ls-remote", "--heads", "origin", "feature"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
    // main 不受影响
    assert_eq!(
        rev_parse(&clone_b, "origin/main"),
        rev_parse(&clone_b, "main")
    );

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn remote_branch_delete_rejects_name_without_remote() {
    let dir = temp_dir("rdel-invalid");
    init_repo(&dir);

    // 无 '/' 或段为空时无法判定远端,报 git_branch_name_required
    for name in ["main", "", "/main", "origin/"] {
        let err = remote_branch_delete_blocking(dir.to_str().unwrap(), name).unwrap_err();
        assert!(
            err.is_code(ErrorCode::GitBranchNameRequired),
            "输入 {name:?} 实际输出: {err}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn list_branches_reports_upstream_tracking() {
    let origin = temp_dir("track-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone_a = temp_dir("track-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);

    // feature 跟踪 origin/main;local-only 无 upstream
    git(&clone_a, &["branch", "--track", "feature", "origin/main"]);
    git(&clone_a, &["branch", "local-only"]);
    // main 本地多一个未推送提交
    fs::write(clone_a.join("a.txt"), "a2").unwrap();
    git(&clone_a, &["commit", "-am", "c2"]);

    // 另一 clone 推进 origin/main,使 main 分叉、feature 落后
    let clone_b = temp_dir("track-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_b, &["config", "user.email", "test@example.com"]);
    git(&clone_b, &["config", "user.name", "test"]);
    fs::write(clone_b.join("b.txt"), "b").unwrap();
    git(&clone_b, &["add", "b.txt"]);
    git(&clone_b, &["commit", "-m", "c3"]);
    git(&clone_b, &["push"]);
    git(&clone_a, &["fetch", "origin"]);

    // aheady 基于最新 origin/main 再提交一个:只领先不落后
    git(&clone_a, &["checkout", "-b", "aheady", "origin/main"]);
    fs::write(clone_a.join("c.txt"), "c").unwrap();
    git(&clone_a, &["add", "c.txt"]);
    git(&clone_a, &["commit", "-m", "c4"]);
    git(&clone_a, &["checkout", "main"]);

    let branches = list_branches_blocking(clone_a.to_str().unwrap()).unwrap();
    let track = |name: &str| branches.tracking.iter().find(|t| t.name == name).cloned();

    let main = track("main").expect("main 应有 tracking");
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (1, 1));

    let feature = track("feature").expect("feature 应有 tracking");
    assert_eq!((feature.ahead, feature.behind), (0, 1));

    let aheady = track("aheady").expect("aheady 应有 tracking");
    assert_eq!((aheady.ahead, aheady.behind), (1, 0));

    assert!(track("local-only").is_none(), "无 upstream 的分支不收录");

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn pull_reports_conflicts() {
    let origin = temp_dir("pull-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);

    let clone_a = temp_dir("pull-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "base\n").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);

    let clone_b = temp_dir("pull-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_b, &["config", "user.email", "test@example.com"]);
    git(&clone_b, &["config", "user.name", "test"]);
    // 显式指定合并策略,避免新版 git 对分叉分支拒绝 pull
    git(&clone_b, &["config", "pull.rebase", "false"]);

    // 双方改同一行 → 合并冲突
    fs::write(clone_a.join("a.txt"), "remote\n").unwrap();
    git(&clone_a, &["commit", "-am", "remote"]);
    git(&clone_a, &["push"]);

    fs::write(clone_b.join("a.txt"), "local\n").unwrap();
    git(&clone_b, &["commit", "-am", "local"]);

    let res = pull_blocking(clone_b.to_str().unwrap()).unwrap();
    assert!(res.status.is_repo);
    assert_eq!(res.conflicts, vec!["a.txt".to_string()]);
    assert_eq!(res.status.conflicted, 1);

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn commit_context_covers_staged_modified_untracked() {
    let dir = temp_dir("ctx");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("a.txt"), "changed").unwrap(); // 未暂存修改
    fs::write(dir.join("b.txt"), "b").unwrap(); // 已暂存新增
    git(&dir, &["add", "b.txt"]);
    fs::write(dir.join("c.txt"), "c").unwrap(); // 未跟踪

    let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
    assert!(ctx.stat.contains("a.txt"));
    assert!(ctx.stat.contains("b.txt"));
    assert!(ctx.diff.contains("changed"));
    assert!(!ctx.truncated);
    assert_eq!(ctx.untracked, vec!["c.txt".to_string()]);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_context_falls_back_to_cached_without_head() {
    let dir = temp_dir("ctx-no-head");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);

    // 尚无提交:回退到暂存区 diff
    let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
    assert!(ctx.diff.contains("a.txt"));
    assert!(ctx.untracked.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

/// 在 dir 下创建一个带一次提交的嵌套 git 仓库
fn init_nested_repo(dir: &PathBuf, name: &str) -> PathBuf {
    let nested = dir.join(name);
    fs::create_dir_all(&nested).unwrap();
    init_repo(&nested);
    fs::write(nested.join("n.txt"), "n").unwrap();
    git(&nested, &["add", "n.txt"]);
    git(&nested, &["commit", "-m", "nested init"]);
    nested
}

#[test]
fn nested_repo_is_not_counted_as_untracked() {
    let dir = temp_dir("status-nested");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    let nested = init_nested_repo(&dir, "sub-lib");
    fs::write(nested.join("n.txt"), "changed").unwrap(); // 嵌套仓库内部改动
    fs::write(dir.join("b.txt"), "b").unwrap(); // 普通未跟踪文件

    // 嵌套仓库及其内部改动都不计入,只有 b.txt 算未跟踪
    let st = status(dir.to_str().unwrap()).unwrap();
    assert_eq!(st.untracked, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_context_excludes_nested_repo() {
    let dir = temp_dir("ctx-nested");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    let nested = init_nested_repo(&dir, "sub-lib");
    fs::write(nested.join("n.txt"), "changed").unwrap();
    fs::write(dir.join("b.txt"), "b").unwrap();

    // 外层只看到 b.txt;嵌套仓库不出现在 untracked,其内部改动不进 diff
    let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(ctx.untracked, vec!["b.txt".to_string()]);
    assert!(ctx.stat.is_empty(), "stat 应为空,实际: {:?}", ctx.stat);
    assert!(ctx.diff.is_empty(), "diff 应为空,实际: {:?}", ctx.diff);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ai_commit_context_follows_untracked_and_path_selection() {
    let dir = temp_dir("ai-ctx-scope");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    fs::write(dir.join("a.txt"), "a changed\n").unwrap();
    fs::write(dir.join("b.txt"), "b changed\n").unwrap();
    fs::write(dir.join("new.txt"), "new content\n").unwrap();

    let without_untracked = ai_commit_context_blocking(dir.to_str().unwrap(), false, None).unwrap();
    assert_eq!(without_untracked.files.len(), 2);
    assert!(!without_untracked.semantic_input.contains("new content"));

    let with_untracked = ai_commit_context_blocking(dir.to_str().unwrap(), true, None).unwrap();
    assert_eq!(with_untracked.files.len(), 3);
    assert!(with_untracked
        .files
        .iter()
        .any(|file| file.path == "new.txt"));
    assert!(with_untracked.semantic_input.contains("new content"));

    let selected = vec!["b.txt".to_string()];
    let partial = ai_commit_context_blocking(dir.to_str().unwrap(), true, Some(&selected)).unwrap();
    assert_eq!(partial.files.len(), 1);
    assert_eq!(partial.files[0].path, "b.txt");
    assert!(!partial.semantic_input.contains("a changed"));
    assert!(!partial.semantic_input.contains("new content"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ai_commit_context_keeps_untracked_binary_metadata() {
    let dir = temp_dir("ai-ctx-binary");
    init_repo(&dir);
    fs::write(dir.join("base.txt"), "base").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    fs::write(dir.join("asset.bin"), [0_u8, 1, 2, 0, 255]).unwrap();

    let context = ai_commit_context_blocking(dir.to_str().unwrap(), true, None).unwrap();
    let binary = context
        .files
        .iter()
        .find(|file| file.path == "asset.bin")
        .unwrap();
    assert!(binary.binary);
    assert!(context.semantic_input.contains("asset.bin"));
    assert!(context.semantic_input.contains("\"beforeContent\":null"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_context_includes_untracked_text_and_skips_binary() {
    let dir = temp_dir("ctx-untracked-content");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("new.txt"), "hello world").unwrap();
    fs::write(dir.join("bin.dat"), [0u8, 159, 146, 150]).unwrap(); // 含 NUL,视为二进制

    let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
    // 名称清单两个都在;内容清单只有文本文件
    assert!(ctx.untracked.contains(&"new.txt".to_string()));
    assert!(ctx.untracked.contains(&"bin.dat".to_string()));
    assert_eq!(ctx.untracked_files.len(), 1);
    assert_eq!(ctx.untracked_files[0].path, "new.txt");
    assert_eq!(ctx.untracked_files[0].content, "hello world");
    assert!(!ctx.untracked_files[0].truncated);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_context_excludes_lockfile_from_diff_but_keeps_stat() {
    let dir = temp_dir("ctx-lockfile");
    init_repo(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), "a").unwrap();
    fs::write(dir.join("sub/pnpm-lock.yaml"), "lockfileVersion: 9").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("a.txt"), "changed").unwrap();
    fs::write(dir.join("sub/pnpm-lock.yaml"), "lockfileVersion: 10").unwrap();

    // diff 排除子目录中的锁文件(* 跨目录匹配);stat 仍保留,模型可感知"锁文件变了"
    let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
    assert!(ctx.diff.contains("changed"));
    assert!(!ctx.diff.contains("pnpm-lock"));
    assert!(ctx.stat.contains("pnpm-lock.yaml"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_context_includes_recent_commits() {
    let dir = temp_dir("ctx-recent");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "feat: init"]);
    fs::write(dir.join("a.txt"), "b").unwrap();
    git(&dir, &["commit", "-am", "fix: second"]);

    // 新提交在前,供模型对齐提交风格
    let ctx = commit_context_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(
        ctx.recent_commits,
        vec!["fix: second".to_string(), "feat: init".to_string()]
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn commit_with_untracked_skips_nested_repo() {
    let dir = temp_dir("commit-nested");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);

    init_nested_repo(&dir, "sub-lib");
    fs::write(dir.join("b.txt"), "b").unwrap();

    // 勾选包含未跟踪:b.txt 被提交,嵌套仓库不被加成 embedded gitlink
    let st = commit_blocking(dir.to_str().unwrap(), "add b", true, None).unwrap();
    assert_eq!(st.untracked, 0);

    let out = git_command(dir.to_str().unwrap())
        .args(["ls-files"])
        .output()
        .unwrap();
    let tracked = String::from_utf8_lossy(&out.stdout);
    assert!(tracked.lines().any(|l| l == "b.txt"));
    assert!(!tracked.lines().any(|l| l.starts_with("sub-lib")));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn git_log_parses_and_filters() {
    let dir = temp_dir("log");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "feat: first"]);
    fs::write(dir.join("a.txt"), "a2").unwrap();
    git(&dir, &["commit", "-am", "fix: second"]);
    // 另一条作者的提交,验证 --author 过滤
    fs::write(dir.join("b.txt"), "b").unwrap();
    git(&dir, &["add", "b.txt"]);
    git(
        &dir,
        &[
            "-c",
            "user.name=other",
            "-c",
            "user.email=other@example.com",
            "commit",
            "-m",
            "docs: third",
        ],
    );

    let all = run_git_log(dir.to_str().unwrap(), None, None, None, None).unwrap();
    assert_eq!(all.len(), 3);
    // 时间倒序:最新在前
    assert_eq!(all[0].subject, "docs: third");
    assert_eq!(all[1].subject, "fix: second");
    assert_eq!(all[2].subject, "feat: first");
    assert_eq!(all[1].author, "test");
    assert!(!all[0].hash.is_empty());

    // author 过滤:仅含匹配作者的提交
    let mine = run_git_log(dir.to_str().unwrap(), None, None, None, Some("test")).unwrap();
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|c| c.author == "test"));
    let nobody = run_git_log(
        dir.to_str().unwrap(),
        None,
        None,
        None,
        Some("no-such-author"),
    )
    .unwrap();
    assert!(nobody.is_empty());

    // max_count 截断
    let one = run_git_log(dir.to_str().unwrap(), None, None, Some(1), None).unwrap();
    assert_eq!(one.len(), 1);

    // until 远早于提交时间 → 空
    let none = run_git_log(dir.to_str().unwrap(), None, Some("2000-01-01"), None, None).unwrap();
    assert!(none.is_empty());

    // 非仓库 → 空数组而非报错
    let plain = temp_dir("log-plain");
    let res = run_git_log(plain.to_str().unwrap(), None, None, None, None).unwrap();
    assert!(res.is_empty());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&plain);
}

#[test]
fn git_current_user_reads_config() {
    let dir = temp_dir("user");
    init_repo(&dir);
    let user = run_git_current_user(dir.to_str().unwrap()).unwrap();
    assert_eq!(user.name, "test");
    assert_eq!(user.email, "test@example.com");

    // 非仓库:不报错即可(字段取决于全局配置,内容不可断言)
    let plain = temp_dir("user-plain");
    run_git_current_user(plain.to_str().unwrap()).unwrap();

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&plain);
}

#[test]
fn graph_log_walks_topo_with_decorations() {
    // 线性历史 + 分支 + 标签:验证拓扑序、refs 装饰、HEAD 标记
    let dir = temp_dir("graph");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "c1"]);
    fs::write(dir.join("a.txt"), "a2").unwrap();
    git(&dir, &["commit", "-am", "c2"]);
    git(&dir, &["branch", "feature"]);
    git(&dir, &["tag", "v1.0"]);

    let repo = open_repo(dir.to_str().unwrap()).unwrap().unwrap();
    let walk = build_graph_revwalk(&repo, None, None).unwrap().unwrap();
    let deco = GraphDeco::collect(&repo);
    let commits: Vec<GitGraphCommit> = walk
        .flatten()
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| deco.commit_entry(&c))
        .collect();

    assert_eq!(commits.len(), 2);
    // 拓扑序:子提交(c2)先于父提交(c1)
    assert_eq!(commits[0].subject, "c2");
    assert_eq!(commits[1].subject, "c1");
    assert_eq!(commits[0].parents, vec![commits[1].hash.clone()]);
    // HEAD -> main 置顶;同提交上的 feature 分支与 tag 装饰一并列出
    assert!(commits[0].is_head);
    assert_eq!(commits[0].refs[0], "main");
    assert!(commits[0].refs.contains(&"feature".to_string()));
    assert!(commits[0].refs.contains(&"tag: v1.0".to_string()));
    assert!(!commits[1].is_head);
    assert!(commits[1].parents.is_empty());

    // 指定分支范围与空仓库的 done 语义
    assert!(
        build_graph_revwalk(&repo, Some(vec!["feature".to_string()]), None)
            .unwrap()
            .is_some()
    );
    assert!(
        build_graph_revwalk(&repo, Some(vec!["no-such".to_string()]), None).is_err(),
        "无法解析的修订名应报错"
    );
    let empty = temp_dir("graph-empty");
    init_repo(&empty);
    let empty_repo = open_repo(empty.to_str().unwrap()).unwrap().unwrap();
    assert!(build_graph_revwalk(&empty_repo, None, None)
        .unwrap()
        .is_none());
    assert!(build_graph_revwalk(&empty_repo, Some(vec![]), None)
        .unwrap()
        .is_none());

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&empty);
}

#[test]
fn graph_log_excludes_remote_refs_when_disabled() {
    let (origin, _clone_a) = setup_origin_with_feature("graph-scope");
    let clone_b = clone_with_config("graph-scope-b", &origin);
    let repo = open_repo(clone_b.to_str().unwrap()).unwrap().unwrap();

    // 默认(全量):含 origin/feature 装饰
    let deco = GraphDeco::collect(&repo);
    let walk = build_graph_revwalk(&repo, None, None).unwrap().unwrap();
    let commits: Vec<GitGraphCommit> = walk
        .flatten()
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| deco.commit_entry(&c))
        .collect();
    let all_refs: Vec<&str> = commits
        .iter()
        .flat_map(|c| c.refs.iter().map(String::as_str))
        .collect();
    assert!(all_refs.contains(&"origin/main"), "实际: {all_refs:?}");
    // origin/HEAD 符号引用不出现在装饰中
    assert!(!all_refs.contains(&"origin/HEAD"), "实际: {all_refs:?}");

    // include_remote=false:只走本地分支+标签,feature 提交不可达
    let walk = build_graph_revwalk(&repo, None, Some(false))
        .unwrap()
        .unwrap();
    let _deco = GraphDeco::collect(&repo);
    let subjects: Vec<String> = walk
        .flatten()
        .filter_map(|oid| repo.find_commit(oid).ok())
        .map(|c| c.summary().unwrap_or_default().to_string())
        .collect();
    assert_eq!(subjects, vec!["c1".to_string()]);

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn worktree_add_and_remove_roundtrip() {
    let dir = temp_dir("worktree");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "hello").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    let path = dir.to_str().unwrap();

    // 初始只有主工作区
    let initial = list_worktrees_blocking(path).unwrap();
    assert_eq!(initial.len(), 1);
    assert!(initial[0].is_main);

    // 相对路径 + {branch} 占位符创建
    let added =
        worktree_add_blocking(path, ".worktrees/{branch}", "feature/x", true, None, None).unwrap();
    assert_eq!(added.len(), 2);
    let wt = added.iter().find(|w| !w.is_main).unwrap();
    assert_eq!(wt.branch.as_deref(), Some("feature/x"));
    assert!(wt.path.replace('\\', "/").contains(".worktrees/feature-x"));
    assert!(Path::new(&wt.path).join("a.txt").exists());

    // 分支已被 worktree 检出 → git_branch_checked_out
    let dup =
        worktree_add_blocking(path, ".worktrees/dup", "feature/x", true, None, None).unwrap_err();
    assert!(dup.is_code(ErrorCode::GitBranchCheckedOut));

    // 挂载已有(未检出)分支
    git(&dir, &["branch", "topic"]);
    let attached =
        worktree_add_blocking(path, ".worktrees/topic", "topic", false, None, None).unwrap();
    assert!(attached
        .iter()
        .any(|w| w.branch.as_deref() == Some("topic")));

    // 挂载已被其它 worktree 检出的分支 → git_branch_checked_out
    let occupied =
        worktree_add_blocking(path, ".worktrees/topic2", "topic", false, None, None).unwrap_err();
    assert!(occupied.is_code(ErrorCode::GitBranchCheckedOut));

    // 主工作区不可删除
    let rm_main = worktree_remove_blocking(path, &initial[0].path, false, false, None).unwrap_err();
    assert!(rm_main.is_code(ErrorCode::GitCommandFailed));

    // 删除 worktree 并删分支
    let left = worktree_remove_blocking(path, &wt.path, false, true, None).unwrap();
    assert_eq!(left.len(), 2);
    assert!(!Path::new(&wt.path).exists());
    assert!(!local_branch_names(path)
        .unwrap()
        .iter()
        .any(|b| b == "feature/x"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn list_worktrees_from_linked_worktree_includes_main() {
    let dir = temp_dir("worktree-from-linked");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "hello").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    let path = dir.to_str().unwrap();

    worktree_add_blocking(path, ".worktrees/feature", "feature", true, None, None).unwrap();
    let wt = dir.join(".worktrees").join("feature");

    // 以链接工作区作为项目路径列 worktree:主工作区仍排第一、分支正确,
    // 链接工作区不重复不遗漏(修复前:副本自身被标为主工作区,真正的主工作区缺失)
    let list = list_worktrees_blocking(wt.to_str().unwrap()).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].is_main);
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    assert_ne!(list[0].path, list[1].path);
    assert!(!list[1].is_main);
    assert_eq!(list[1].branch.as_deref(), Some("feature"));
    assert!(Path::new(&list[1].path).join("a.txt").exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn worktree_remove_prunes_missing_directory() {
    let dir = temp_dir("worktree-prune-missing-directory");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "hello").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    let path = dir.to_str().unwrap();

    let added =
        worktree_add_blocking(path, ".worktrees/feature", "feature", true, None, None).unwrap();
    let wt = added.iter().find(|w| !w.is_main).unwrap();
    fs::remove_dir_all(&wt.path).unwrap();

    let remaining = worktree_remove_blocking(path, &wt.path, false, false, None).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].is_main);
    assert!(!list_worktrees_blocking(path)
        .unwrap()
        .iter()
        .any(|w| w.path == wt.path));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn worktree_inside_repo_excluded_from_untracked() {
    let dir = temp_dir("worktree-exclude");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a").unwrap();
    git(&dir, &["add", "a.txt"]);
    git(&dir, &["commit", "-m", "init"]);
    let path = dir.to_str().unwrap();

    // 创建工作区内的 worktree 前:.worktrees/ 是一条未跟踪目录(嵌套仓库边界,
    // 提交对话框排除它,与状态计数不一致)
    worktree_add_blocking(path, ".worktrees/feature", "feature", true, None, None).unwrap();
    let exclude = fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.contains("/.worktrees/feature/"),
        "exclude: {exclude}"
    );
    let st = status(path).unwrap();
    assert_eq!(st.untracked, 0, "worktree 目录不应计入未跟踪");

    // 幂等:再次列出 worktree 不重复追加
    list_worktrees_blocking(path).unwrap();
    let exclude2 = fs::read_to_string(dir.join(".git/info/exclude")).unwrap();
    assert_eq!(
        exclude2.matches("/.worktrees/feature/").count(),
        1,
        "exclude: {exclude2}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn worktree_add_remote_branch_tracks_or_aligns_local() {
    // origin:clone_a 推 main 及 feature/topic/hotfix/ahead 四个远程分支(各含 v1 提交)
    let origin = temp_dir("wt-remote-origin");
    git(&origin, &["init", "--bare", "-b", "main"]);
    let clone_a = temp_dir("wt-remote-a");
    git(&clone_a, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_a, &["config", "user.email", "test@example.com"]);
    git(&clone_a, &["config", "user.name", "test"]);
    fs::write(clone_a.join("a.txt"), "a").unwrap();
    git(&clone_a, &["add", "a.txt"]);
    git(&clone_a, &["commit", "-m", "c1"]);
    git(&clone_a, &["push", "-u", "origin", "main"]);
    for (name, file) in [
        ("feature", "f.txt"),
        ("topic", "t.txt"),
        ("hotfix", "h.txt"),
        ("ahead", "ah.txt"),
    ] {
        git(&clone_a, &["checkout", "-b", name, "main"]);
        fs::write(clone_a.join(file), "v1").unwrap();
        git(&clone_a, &["add", file]);
        git(&clone_a, &["commit", "-m", &format!("{name}1")]);
        git(&clone_a, &["push", "-u", "origin", name]);
    }

    let clone_b = temp_dir("wt-remote-b");
    git(&clone_b, &["clone", origin.to_str().unwrap(), "."]);
    git(&clone_b, &["config", "user.email", "test@example.com"]);
    git(&clone_b, &["config", "user.name", "test"]);
    let path_b = clone_b.to_str().unwrap();

    // 1. 本地无同名分支:挂载 origin/feature → 显式创建跟踪分支(而非游离 HEAD)
    let list = worktree_add_blocking(
        path_b,
        ".worktrees/feature",
        "origin/feature",
        false,
        None,
        None,
    )
    .unwrap();
    let wt = list.iter().find(|w| !w.is_main).unwrap();
    assert_eq!(wt.branch.as_deref(), Some("feature"));
    assert!(!wt.detached);
    assert!(Path::new(&wt.path).join("f.txt").exists());
    let up = git_command(path_b)
        .args(["rev-parse", "--abbrev-ref", "feature@{upstream}"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&up.stdout).trim(), "origin/feature");

    // 远程引用落地名已被 worktree 检出 → git_branch_checked_out
    let occupied = worktree_add_blocking(
        path_b,
        ".worktrees/feature2",
        "origin/feature",
        false,
        None,
        None,
    )
    .unwrap_err();
    assert!(occupied.is_code(ErrorCode::GitBranchCheckedOut));

    // 2. 本地同名分支落后于远程:先快进对齐到远程提交再挂载
    git(&clone_b, &["branch", "topic", "origin/topic"]);
    git(&clone_a, &["checkout", "topic"]);
    fs::write(clone_a.join("t2.txt"), "t2").unwrap();
    git(&clone_a, &["add", "t2.txt"]);
    git(&clone_a, &["commit", "-m", "t2"]);
    git(&clone_a, &["push", "origin", "topic"]);
    git(&clone_b, &["fetch", "origin"]);
    let list = worktree_add_blocking(
        path_b,
        ".worktrees/topic",
        "origin/topic",
        false,
        None,
        None,
    )
    .unwrap();
    let wt = list
        .iter()
        .find(|w| w.branch.as_deref() == Some("topic"))
        .unwrap();
    // 远程新提交在 worktree 中可见,且本地 topic 已对齐 origin/topic
    assert!(Path::new(&wt.path).join("t2.txt").exists());
    let local_rev = git_command(path_b)
        .args(["rev-parse", "topic"])
        .output()
        .unwrap();
    let remote_rev = git_command(path_b)
        .args(["rev-parse", "origin/topic"])
        .output()
        .unwrap();
    assert_eq!(local_rev.stdout, remote_rev.stdout);

    // 3. 本地同名分支与远程分叉:报 git_branch_diverged,不静默重置丢本地提交
    git(&clone_b, &["checkout", "-b", "hotfix", "origin/hotfix"]);
    fs::write(clone_b.join("local-only.txt"), "l").unwrap();
    git(&clone_b, &["add", "local-only.txt"]);
    git(&clone_b, &["commit", "-m", "local-only"]);
    git(&clone_b, &["checkout", "main"]);
    git(&clone_a, &["checkout", "hotfix"]);
    fs::write(clone_a.join("h2.txt"), "h2").unwrap();
    git(&clone_a, &["add", "h2.txt"]);
    git(&clone_a, &["commit", "-m", "h2"]);
    git(&clone_a, &["push", "origin", "hotfix"]);
    git(&clone_b, &["fetch", "origin"]);
    let err = worktree_add_blocking(
        path_b,
        ".worktrees/hotfix",
        "origin/hotfix",
        false,
        None,
        None,
    )
    .unwrap_err();
    assert!(err.is_code(ErrorCode::GitBranchDiverged));

    // 4. 本地同名分支领先远程(远程是其祖先):直接挂载本地分支,保留本地提交
    git(&clone_b, &["checkout", "-b", "ahead", "origin/ahead"]);
    fs::write(clone_b.join("ahead-local.txt"), "l").unwrap();
    git(&clone_b, &["add", "ahead-local.txt"]);
    git(&clone_b, &["commit", "-m", "ahead-local"]);
    git(&clone_b, &["checkout", "main"]);
    let list = worktree_add_blocking(
        path_b,
        ".worktrees/ahead",
        "origin/ahead",
        false,
        None,
        None,
    )
    .unwrap();
    let wt = list
        .iter()
        .find(|w| w.branch.as_deref() == Some("ahead"))
        .unwrap();
    assert!(Path::new(&wt.path).join("ahead-local.txt").exists());

    let _ = fs::remove_dir_all(&origin);
    let _ = fs::remove_dir_all(&clone_a);
    let _ = fs::remove_dir_all(&clone_b);
}

#[test]
fn project_stats_aggregates_history_churn_and_file_types() {
    let dir = temp_dir("project-stats");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    // 分支提交,稍后 --no-ff 合并产生一个合并提交
    git(&dir, &["checkout", "-b", "feature"]);
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "feature"]);
    git(&dir, &["checkout", "main"]);
    fs::write(dir.join("a.txt"), "a1\na2\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "main work"]);
    git(
        &dir,
        &["merge", "--no-ff", "feature", "-m", "merge feature"],
    );

    let stats = collect_project_stats(dir.to_str().unwrap()).unwrap();
    assert_eq!(stats.total_commits, 4);
    assert_eq!(stats.merge_commits, 1);
    // churn: init +1/-0,feature +1/-0,main work +2/-1;合并提交不计
    assert_eq!(stats.total_additions, 4);
    assert_eq!(stats.total_deletions, 1);
    assert!(!stats.churn_truncated);
    assert_eq!(stats.authors.len(), 1);
    assert_eq!(stats.authors[0].commits, 4);
    assert_eq!(stats.authors[0].additions, 4);
    assert_eq!(stats.authors[0].deletions, 1);
    assert_eq!(stats.active_days, stats.by_day.len() as u64);
    // 星期×小时与按日两个桶的合计都应等于总提交数
    let bucketed: u32 = stats.weekday_hour.iter().sum();
    assert_eq!(u64::from(bucketed), stats.total_commits);
    let daily: u32 = stats.by_day.iter().map(|d| d.count).sum();
    assert_eq!(u64::from(daily), stats.total_commits);
    // 首末时间与按日 churn 聚合
    assert!(stats.first_commit_at.is_some());
    assert_eq!(
        stats
            .first_commit_at
            .zip(stats.last_commit_at)
            .map(|(f, l)| f <= l),
        Some(true)
    );
    let adds: u64 = stats.by_day.iter().map(|d| d.additions).sum();
    assert_eq!(adds, stats.total_additions);
    // 逐提交 churn:合并提交不进明细,合计与总数一致,按时间升序
    assert_eq!(stats.churn_commits.len(), 3);
    let adds: u64 = stats.churn_commits.iter().map(|c| c.additions).sum();
    let dels: u64 = stats.churn_commits.iter().map(|c| c.deletions).sum();
    assert_eq!((adds, dels), (stats.total_additions, stats.total_deletions));
    assert!(stats.churn_commits.windows(2).all(|w| w[0].t <= w[1].t));
    assert!(stats.churn_commits.iter().all(|c| c.short_id.len() == 7));
    let mut subjects: Vec<&str> = stats
        .churn_commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    subjects.sort_unstable();
    assert_eq!(subjects, ["feature", "init", "main work"]);
    // HEAD 树文件类型:a.txt / b.txt 归 "txt"
    let txt = stats
        .file_types
        .iter()
        .find(|f| f.ext == "txt")
        .expect("txt 应在文件类型分布中");
    assert_eq!(txt.files, 2);
    assert_eq!(stats.total_files, 2);
    assert!(stats.total_bytes >= 4);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn project_stats_rejects_non_repo_and_allows_empty_repo() {
    let dir = temp_dir("project-stats-empty");
    assert!(collect_project_stats(dir.to_str().unwrap()).is_err());
    init_repo(&dir);
    let stats = collect_project_stats(dir.to_str().unwrap()).unwrap();
    assert_eq!(stats.total_commits, 0);
    assert!(stats.authors.is_empty());
    assert_eq!(stats.weekday_hour.len(), 168);
    assert!(stats.file_types.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn project_stats_merges_authors_by_email_case_insensitive() {
    let dir = temp_dir("project-stats-authors");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    // 同一 email 不同大小写/名字:归并为一人
    git(&dir, &["config", "user.email", "TEST@example.com"]);
    git(&dir, &["config", "user.name", "Test Renamed"]);
    fs::write(dir.join("a.txt"), "a1\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "second"]);

    let stats = collect_project_stats(dir.to_str().unwrap()).unwrap();
    assert_eq!(stats.authors.len(), 1);
    assert_eq!(stats.authors[0].commits, 2);
    // 展示名取最近一次(遍历新→旧的首次)出现的名字
    assert_eq!(stats.authors[0].name, "Test Renamed");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stash_list_uses_git2_and_writes_pop_or_drop_selected_entry() {
    let dir = temp_dir("stash-management");
    init_repo(&dir);
    fs::write(dir.join("a.txt"), "base\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    fs::write(dir.join("a.txt"), "first\n").unwrap();
    git(&dir, &["stash", "push", "-m", "first stash"]);
    fs::write(dir.join("a.txt"), "second\n").unwrap();
    git(&dir, &["stash", "push", "-m", "second stash"]);

    let listed = list_stashes_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].index, 0);
    assert_eq!(listed[1].index, 1);
    assert_eq!(listed[0].message, "On main: second stash");
    assert_eq!(listed[1].message, "On main: first stash");
    assert_eq!(listed[0].author, "test");
    assert!(listed.iter().all(|stash| stash.created_at > 0));

    // index 与 oid 不匹配时拒绝执行，避免列表变化后误操作另一条 stash。
    let stale = stash_write_blocking(
        dir.to_str().unwrap(),
        listed[0].index,
        &listed[1].oid,
        "drop",
    )
    .unwrap_err();
    assert!(stale.is_code(ErrorCode::GitStashChanged));

    // 按需清理最新一条后，旧记录前移到 stash@{0}。
    let clean = stash_write_blocking(
        dir.to_str().unwrap(),
        listed[0].index,
        &listed[0].oid,
        "drop",
    )
    .unwrap();
    assert_eq!(clean.modified, 0);
    let remaining = list_stashes_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].index, 0);
    assert_eq!(remaining[0].oid, listed[1].oid);

    // 弹出指定记录会应用内容并移除该记录。
    let popped = stash_write_blocking(
        dir.to_str().unwrap(),
        remaining[0].index,
        &remaining[0].oid,
        "pop",
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("a.txt")).unwrap().trim_end(),
        "first"
    );
    assert_eq!(popped.modified, 1);
    assert!(list_stashes_blocking(dir.to_str().unwrap())
        .unwrap()
        .is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stash_list_rejects_non_repository() {
    let dir = temp_dir("stash-non-repo");
    let error = list_stashes_blocking(dir.to_str().unwrap()).unwrap_err();
    assert!(error.is_code(ErrorCode::NotGitRepository));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stash_push_creates_entry_and_optionally_includes_untracked_files() {
    let dir = temp_dir("stash-push");
    init_repo(&dir);
    fs::write(dir.join("tracked.txt"), "base\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);

    let clean = stash_push_blocking(dir.to_str().unwrap(), "", false).unwrap_err();
    assert!(clean.is_code(ErrorCode::GitStashNothingToSave));

    fs::write(dir.join("tracked.txt"), "changed\n").unwrap();
    let status = stash_push_blocking(dir.to_str().unwrap(), "tracked changes", false).unwrap();
    assert_eq!(status.modified, 0);
    let listed = list_stashes_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].message, "On main: tracked changes");

    fs::write(dir.join("untracked.txt"), "new\n").unwrap();
    let excluded = stash_push_blocking(dir.to_str().unwrap(), "untracked", false).unwrap_err();
    assert!(excluded.is_code(ErrorCode::GitStashNothingToSave));
    assert!(dir.join("untracked.txt").exists());

    let status = stash_push_blocking(dir.to_str().unwrap(), "untracked", true).unwrap();
    assert_eq!(status.untracked, 0);
    assert!(!dir.join("untracked.txt").exists());
    let listed = list_stashes_blocking(dir.to_str().unwrap()).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].message, "On main: untracked");

    let untracked_files = stash_files_blocking(dir.to_str().unwrap(), &listed[0].oid).unwrap();
    assert_eq!(untracked_files.len(), 1);
    assert_eq!(untracked_files[0].path, "untracked.txt");
    assert_eq!(untracked_files[0].status, "A");
    assert_eq!(untracked_files[0].additions, Some(1));
    let untracked_diff = stash_file_diff_blocking(
        dir.to_str().unwrap(),
        &listed[0].oid,
        "untracked.txt",
        None,
        None,
    )
    .unwrap();
    assert!(untracked_diff.diff.contains("+new"));

    let tracked_files = stash_files_blocking(dir.to_str().unwrap(), &listed[1].oid).unwrap();
    assert_eq!(tracked_files.len(), 1);
    assert_eq!(tracked_files[0].path, "tracked.txt");
    assert_eq!(tracked_files[0].status, "M");
    let tracked_diff = stash_file_diff_blocking(
        dir.to_str().unwrap(),
        &listed[1].oid,
        "tracked.txt",
        None,
        None,
    )
    .unwrap();
    assert!(tracked_diff.diff.contains("-base"));
    assert!(tracked_diff.diff.contains("+changed"));

    let _ = fs::remove_dir_all(&dir);
}
