use super::super::*;

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

#[test]
fn unshallow_restores_history_and_all_remote_branches() {
    use super::helpers::{git, init_repo, temp_dir};
    let origin = temp_dir("unshallow-origin");
    init_repo(&origin);
    git(&origin, &["commit", "--allow-empty", "-m", "first"]);
    git(&origin, &["commit", "--allow-empty", "-m", "second"]);
    git(&origin, &["branch", "feature"]);
    let target = temp_dir("unshallow-target");
    let url = url::Url::from_directory_path(&origin).unwrap().to_string();
    git(&target, &["clone", "--depth", "1", &url, "."]);
    let path = target.to_str().unwrap();
    assert!(status(path).unwrap().is_shallow);
    assert!(run_git(path, &["rev-parse", "--verify", "origin/feature"]).is_err());
    let head = run_git(path, &["rev-parse", "HEAD"]).unwrap().stdout;
    std::fs::write(target.join("local.txt"), "keep me").unwrap();
    assert!(!unshallow_blocking(path).unwrap().is_shallow);
    assert_eq!(run_git(path, &["rev-list", "--count", "HEAD"]).unwrap().stdout, b"2\n");
    assert!(run_git(path, &["rev-parse", "--verify", "origin/feature"]).is_ok());
    assert_eq!(run_git(path, &["rev-parse", "HEAD"]).unwrap().stdout, head);
    assert_eq!(std::fs::read_to_string(target.join("local.txt")).unwrap(), "keep me");
    assert!(!unshallow_blocking(path).unwrap().is_shallow);
    assert_eq!(String::from_utf8(run_git(path, &["config", "--get-all", "remote.origin.fetch"]).unwrap().stdout).unwrap().trim(), "+refs/heads/*:refs/remotes/origin/*");
}
