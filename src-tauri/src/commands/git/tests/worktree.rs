use super::super::*;
use super::helpers::*;
use std::fs;

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

