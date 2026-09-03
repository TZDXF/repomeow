use super::super::*;
use super::helpers::*;
use std::fs;

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

