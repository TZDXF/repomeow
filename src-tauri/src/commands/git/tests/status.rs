use super::super::*;
use super::helpers::*;
use std::fs;

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

