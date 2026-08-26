use super::default_schedule_name;

#[test]
fn default_schedule_name_follows_ui_language() {
    // 中文(zh-CN 或其他未识别语言):周报/日报
    assert_eq!(default_schedule_name(true, "zh-CN"), "周报定时任务");
    assert_eq!(default_schedule_name(false, "zh-CN"), "日报定时任务");
    assert_eq!(default_schedule_name(true, "ja-JP"), "周报定时任务");
    // 英文(en-US):Weekly / Daily Schedule
    assert_eq!(default_schedule_name(true, "en-US"), "Weekly Schedule");
    assert_eq!(default_schedule_name(false, "en-US"), "Daily Schedule");
}

#[test]
fn daily_report_date_is_previous_day() {
    // 次日生成(默认):8/20 触发 → 报告日 8/19
    assert_eq!(
        super::daily_report_date(chrono::NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(), true),
        chrono::NaiveDate::from_ymd_opt(2026, 8, 19).unwrap()
    );
    // 跨月:8/1 触发 → 报告日 7/31
    assert_eq!(
        super::daily_report_date(chrono::NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(), true),
        chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap()
    );
    // 当天生成:报告日 = 触发日
    let fire = chrono::NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
    assert_eq!(super::daily_report_date(fire, false), fire);
}

#[test]
fn daily_filters_apply_to_report_date_not_fire_date() {
    use chrono::Datelike;
    let all_days = &|_: chrono::NaiveDate| true;
    // 报告日为周五(触发日周六):「仅周一至周五」仍生成 —— 周五晚加班提交不丢
    let fri = chrono::NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
    assert_eq!(fri.weekday(), chrono::Weekday::Fri);
    assert!(super::daily_filters_allow(fri, true, false, all_days));
    // 报告日为周六(触发日周日):「仅周一至周五」跳过
    let sat = chrono::NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
    assert_eq!(sat.weekday(), chrono::Weekday::Sat);
    assert!(!super::daily_filters_allow(sat, true, false, all_days));
    // 「仅中国工作日」:报告日为节假日跳过,为工作日(含调休补班)生成
    let except_fri = &|d: chrono::NaiveDate| d != fri;
    assert!(!super::daily_filters_allow(fri, false, true, except_fri));
    assert!(super::daily_filters_allow(sat, false, true, all_days));
    // 不过滤:任意报告日都生成
    assert!(super::daily_filters_allow(sat, false, false, all_days));
}

use crate::commands::report::read_schedules;
use crate::db;

fn test_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    conn
}

fn insert_schedule(conn: &rusqlite::Connection, id: &str) {
    conn.execute(
        "INSERT INTO report_schedules (id, name, enabled, report_type, project_ids,
             author_mode, time_of_day, weekdays_only, chinese_workday_only,
             weekly_workweek, weekly_start_weekday, weekly_end_weekday, last_run_at)
         VALUES (?1, '', 1, 'daily', '[]', 'me', '09:00', 0, 0, 1, 1, 5, NULL)",
        rusqlite::params![id],
    )
    .unwrap();
}

#[test]
fn mark_last_run_writes_timestamp_on_success() {
    // 正常路径:写入 last_run_at,函数返回 Ok
    let conn = test_conn();
    insert_schedule(&conn, "s-ok");

    let result = super::mark_last_run(&conn, "s-ok");
    assert!(result.is_ok(), "正常写入应成功: {result:?}");

    let schedules = read_schedules(&conn).unwrap();
    let s = schedules
        .iter()
        .find(|s| s.id == "s-ok")
        .expect("schedule 存在");
    let ts = s.last_run_at.expect("last_run_at 应被更新");
    let now = chrono::Local::now().timestamp();
    // 允许 ±2s 抖动
    assert!(
        (ts - now).abs() <= 2,
        "last_run_at 应接近当前时间(差值={})",
        ts - now
    );
}

#[test]
fn mark_last_run_returns_error_on_missing_schedule() {
    // 不插入 schedule,确保更新命中零行
    let conn = test_conn();
    let result = super::mark_last_run(&conn, "nonexistent");
    assert!(
        result.is_err(),
        "对不存在的 schedule_id 调用 mark_last_run 应返回 Err"
    );
}

#[test]
fn mark_last_run_no_silent_recovery_on_zero_row_update() {
    // 不插入 schedule,确保更新命中零行
    let conn = test_conn();
    let result = super::mark_last_run(&conn, "missing-schedule");
    assert!(
        result.is_err(),
        "对不存在的 schedule 调用 mark_last_run 应返回 Err(防止 last_run_at 未更新却 emit)"
    );
}

#[test]
fn mark_last_run_persists_timestamp_visible_via_read_schedules() {
    let conn = test_conn();
    insert_schedule(&conn, "s-1");
    let result = super::mark_last_run(&conn, "s-1");
    assert!(result.is_ok());

    let schedules = read_schedules(&conn).unwrap();
    let s = schedules.iter().find(|s| s.id == "s-1").unwrap();
    assert!(s.last_run_at.is_some(), "last_run_at 应被持久化");
}

use crate::commands::report::delete_report_history_impl;

fn insert_history_row(conn: &rusqlite::Connection) -> i64 {
    let now = crate::time_util::now_ts();
    conn.execute(
        "INSERT INTO report_history (project_ids, date_from, date_to, range_label,
             author_mode, language, period_type, result, created_at)
         VALUES ('[]', '2026-07-01', '2026-07-01', '', 'me', 'zh-CN', 'daily', '', ?1)",
        rusqlite::params![now],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn count_history(conn: &rusqlite::Connection) -> i64 {
    conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM report_history", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn delete_report_history_row_removes_existing_row() {
    let conn = test_conn();
    let id = insert_history_row(&conn);
    assert_eq!(count_history(&conn), 1);

    let res = super::delete_report_history_row(&conn, id);
    assert!(res.is_ok(), "删除存在的行应成功: {res:?}");
    assert_eq!(count_history(&conn), 0, "行应已被删除");
}

#[test]
fn delete_report_history_row_returns_err_for_missing_id() {
    let conn = test_conn();
    let res = super::delete_report_history_row(&conn, 999_999);
    assert!(res.is_err(), "删除不存在的 id 应返回 Err");
}

#[test]
fn delete_report_history_row_via_command_cascades_commits() {
    // 通过命令实现验证 report_commits 的外键级联删除
    let conn = test_conn();
    let history_id = insert_history_row(&conn);
    // 插入一条关联 commit 行,模拟 fire_schedule 写入的子表
    let commits_json =
        r#"[{"hash":"abc","author":"tester","date":"2026-07-01 09:00","subject":"x"}]"#;
    conn.execute(
        "INSERT INTO report_commits (report_id, project_id, project_name,
             project_description, commit_data)
         VALUES (?1, NULL, '', '', ?2)",
        rusqlite::params![history_id, commits_json],
    )
    .unwrap();
    assert_eq!(count_history(&conn), 1);

    delete_report_history_impl(&conn, history_id).unwrap();
    assert_eq!(count_history(&conn), 0);
    let commit_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM report_commits WHERE report_id = ?1",
            rusqlite::params![history_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(commit_count, 0, "report_commits 应随外键级联删除");
}
