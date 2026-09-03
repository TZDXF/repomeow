use super::super::*;
use std::fs;
use std::path::PathBuf;

pub(super) fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "repomeow-git-test-{tag}-{}-{}",
        std::process::id(),
        crate::time_util::now_ts_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn git(dir: &PathBuf, args: &[&str]) {
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

pub(super) fn init_repo(dir: &PathBuf) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "test"]);
}

pub(super) fn clone_with_config(tag: &str, origin: &PathBuf) -> PathBuf {
    let dir = temp_dir(tag);
    git(&dir, &["clone", origin.to_str().unwrap(), "."]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "test"]);
    dir
}

pub(super) fn rev_parse(dir: &PathBuf, rev: &str) -> String {
    let out = git_command(dir.to_str().unwrap())
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    assert!(out.status.success(), "rev-parse {rev} 失败");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// origin(bare) + clone_a:首推 main,并创建推送 feature 分支(clone_a 停留在 feature)
pub(super) fn setup_origin_with_feature(tag: &str) -> (PathBuf, PathBuf) {
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

pub(super) fn init_nested_repo(dir: &PathBuf, name: &str) -> PathBuf {
    let nested = dir.join(name);
    fs::create_dir_all(&nested).unwrap();
    init_repo(&nested);
    fs::write(nested.join("n.txt"), "n").unwrap();
    git(&nested, &["add", "n.txt"]);
    git(&nested, &["commit", "-m", "nested init"]);
    nested
}

