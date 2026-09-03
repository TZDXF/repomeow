use super::super::*;
use super::helpers::*;
use std::fs;

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

