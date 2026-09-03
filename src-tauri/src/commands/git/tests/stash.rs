use super::super::*;
use super::helpers::*;
use std::fs;

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
