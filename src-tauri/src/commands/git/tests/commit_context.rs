use super::super::*;
use super::helpers::*;
use std::fs;

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

/// 取某文件进入提示词前的 raw_patch(文件不存在时为 None)
fn file_patch<'a>(files: &'a [AiCommitFileContext], path: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.raw_patch.as_str())
}

/// 在 dir 下创建一个带一次提交的嵌套 git 仓库
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
    assert!(
        !file_patch(&without_untracked.files, "new.txt").is_some_and(|p| p.contains("new content"))
    );

    let with_untracked = ai_commit_context_blocking(dir.to_str().unwrap(), true, None).unwrap();
    assert_eq!(with_untracked.files.len(), 3);
    assert!(with_untracked
        .files
        .iter()
        .any(|file| file.path == "new.txt"));
    assert!(file_patch(&with_untracked.files, "new.txt")
        .unwrap()
        .contains("new content"));

    let selected = vec!["b.txt".to_string()];
    let partial = ai_commit_context_blocking(dir.to_str().unwrap(), true, Some(&selected)).unwrap();
    assert_eq!(partial.files.len(), 1);
    assert_eq!(partial.files[0].path, "b.txt");
    assert!(!partial.files[0].raw_patch.contains("a changed"));
    assert!(!partial.files[0].raw_patch.contains("new content"));

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
    assert!(context.stat.contains("asset.bin | binary"));
    // 二进制只保留元数据占位,不把字节内容塞进 raw_patch
    assert_eq!(
        binary.raw_patch,
        "diff --git a/asset.bin b/asset.bin\nnew file mode 100644\n\
         Binary files /dev/null and b/asset.bin differ\n"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ai_commit_context_marks_lockfile_as_excluded_but_keeps_summary() {
    let dir = temp_dir("ai-ctx-lockfile");
    init_repo(&dir);
    fs::write(dir.join("Cargo.lock"), "version = 3\n").unwrap();
    fs::write(dir.join("main.rs"), "fn main() {}\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    fs::write(
        dir.join("Cargo.lock"),
        "version = 3\n[[package]]\nname = \"probe\"\n",
    )
    .unwrap();
    fs::write(dir.join("main.rs"), "fn main() { println!(\"probe\"); }\n").unwrap();

    let context = ai_commit_context_blocking(dir.to_str().unwrap(), false, None).unwrap();
    assert!(context.stat.contains("Cargo.lock | M"));
    assert!(context.stat.contains("main.rs | M"));
    let lock = context
        .files
        .iter()
        .find(|file| file.path == "Cargo.lock")
        .unwrap();
    assert!(lock.raw_excluded);
    let code = context
        .files
        .iter()
        .find(|file| file.path == "main.rs")
        .unwrap();
    assert!(!code.raw_excluded);
    assert!(code.raw_patch.contains("diff --git a/main.rs b/main.rs"));

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
