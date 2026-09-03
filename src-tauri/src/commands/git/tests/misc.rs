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

