#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::GitCommitInfo;
    use rusqlite::{params, Connection};

    /// 创建内存 SQLite(应用所有迁移),用于报告测试
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    /// 直接向 projects 表插入一行(绕过 `add` 的目录存在检查)
    fn insert_project(conn: &Connection, name: &str) -> i64 {
        let now = crate::time_util::now_ts();
        conn.execute(
            "INSERT INTO projects (path, name, description, created_at, updated_at)
             VALUES (?1, ?2, '', ?3, ?3)",
            params![format!("/tmp/{name}-{now}"), name, now],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_tag(conn: &Connection, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO tags (name, color) VALUES (?1, '#000000')",
            params![name],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn tag_project(conn: &Connection, project_id: i64, tag_id: i64) {
        conn.execute(
            "INSERT INTO project_tags (project_id, tag_id) VALUES (?1, ?2)",
            params![project_id, tag_id],
        )
        .unwrap();
    }

    #[test]
    fn tag_project_ids_matches_any_selected_tag() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        let c = insert_project(&conn, "gamma");
        let t1 = insert_tag(&conn, "t1");
        let t2 = insert_tag(&conn, "t2");
        tag_project(&conn, a, t1);
        tag_project(&conn, b, t2);
        tag_project(&conn, c, t1);
        tag_project(&conn, c, t2);

        // 任一标签命中即纳入;c 同时含两个标签只出现一次(按 id 排序)
        assert_eq!(tag_project_ids(&conn, &[t1, t2]).unwrap(), vec![a, b, c]);
        assert_eq!(tag_project_ids(&conn, &[t1]).unwrap(), vec![a, c]);
        // 空标签列表 → 空结果
        assert_eq!(tag_project_ids(&conn, &[]).unwrap(), Vec::<i64>::new());
    }

    #[test]
    fn tag_project_ids_excludes_archived_projects() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let t1 = insert_tag(&conn, "t1");
        tag_project(&conn, a, t1);
        conn.execute(
            "UPDATE projects SET archived_at = 1 WHERE id = ?1",
            params![a],
        )
        .unwrap();
        assert_eq!(tag_project_ids(&conn, &[t1]).unwrap(), Vec::<i64>::new());
    }

    #[test]
    fn read_schedules_parses_tag_ids() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO report_schedules (id, name, enabled, report_type, project_ids, tag_ids,
                 author_mode, time_of_day, weekdays_only, chinese_workday_only,
                 weekly_workweek, weekly_start_weekday, weekly_end_weekday, last_run_at)
             VALUES ('s1', '', 1, 'daily', '[1,2]', '[3,4]', 'me', '09:00', 0, 0, 1, 1, 5, NULL)",
            [],
        )
        .unwrap();
        let schedules = read_schedules(&conn).unwrap();
        assert_eq!(schedules[0].project_ids, vec![1, 2]);
        assert_eq!(schedules[0].tag_ids, vec![3, 4]);
    }

    /// 插入报告 + 关联 commits;commits_per_record 表示每个 project 的 commit 数。
    /// created_at 显式传入,避免连续插入时 created_at 相同导致排序无法断言。
    fn insert_report(
        conn: &Connection,
        project_ids: &[i64],
        date_from: &str,
        date_to: &str,
        period_type: &str,
        commits_per_record: usize,
    ) -> i64 {
        insert_report_with_ts(
            conn,
            project_ids,
            date_from,
            date_to,
            period_type,
            commits_per_record,
            crate::time_util::now_ts(),
        )
    }

    /// 与 `insert_report` 类似,但允许指定 `created_at` 以便稳定断言排序。
    fn insert_report_with_ts(
        conn: &Connection,
        project_ids: &[i64],
        date_from: &str,
        date_to: &str,
        period_type: &str,
        commits_per_record: usize,
        created_at: i64,
    ) -> i64 {
        let ids_json = serde_json::to_string(project_ids).unwrap();
        conn.execute(
            "INSERT INTO report_history (project_ids, date_from, date_to, range_label,
                 author_mode, language, period_type, result, created_at)
             VALUES (?1, ?2, ?3, '', 'me', 'zh-CN', ?4, '', ?5)",
            params![ids_json, date_from, date_to, period_type, created_at],
        )
        .unwrap();
        let report_id = conn.last_insert_rowid();
        for pid in project_ids {
            let commits: Vec<GitCommitInfo> = (0..commits_per_record)
                .map(|i| GitCommitInfo {
                    hash: format!("h{i}"),
                    author: "tester".into(),
                    date: "2026-07-01 09:00".into(),
                    subject: format!("commit {i}"),
                })
                .collect();
            let commit_data = serde_json::to_string(&commits).unwrap();
            conn.execute(
                "INSERT INTO report_commits (report_id, project_id, project_name,
                     project_description, commit_data)
                 VALUES (?1, ?2, '', '', ?3)",
                params![report_id, pid, commit_data],
            )
            .unwrap();
        }
        report_id
    }

    #[test]
    fn list_filters_exact_project_id_not_substring() {
        // json_each 按 JSON 数组元素精确匹配项目 ID
        let conn = test_conn();
        let p1 = insert_project(&conn, "p1");
        let p12 = insert_project(&conn, "p12");
        let p123 = insert_project(&conn, "p123");

        insert_report(&conn, &[p1], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p12], "2026-07-02", "2026-07-02", "daily", 2);
        insert_report(&conn, &[p123], "2026-07-03", "2026-07-03", "daily", 3);

        let only_p12 = list_report_history_impl(&conn, None, None, Some(p12)).unwrap();
        assert_eq!(
            only_p12.len(),
            1,
            "筛选 12 必须只返回一条记录(不应包含 1/123)"
        );
        assert_eq!(only_p12[0].project_ids, vec![p12]);
        assert_eq!(only_p12[0].project_names, vec!["p12".to_string()]);
        assert_eq!(only_p12[0].total_commits, 2);

        let only_p1 = list_report_history_impl(&conn, None, None, Some(p1)).unwrap();
        assert_eq!(only_p1.len(), 1);
        assert_eq!(only_p1[0].project_ids, vec![p1]);

        let only_p123 = list_report_history_impl(&conn, None, None, Some(p123)).unwrap();
        assert_eq!(only_p123.len(), 1);
        assert_eq!(only_p123[0].project_ids, vec![p123]);

        // 不筛选应返回全部
        let all = list_report_history_impl(&conn, None, None, None).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn list_returns_descending_by_created_at() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        let r1 = insert_report_with_ts(
            &conn,
            &[p],
            "2026-07-01",
            "2026-07-01",
            "daily",
            1,
            1_000_000,
        );
        let r2 = insert_report_with_ts(
            &conn,
            &[p],
            "2026-07-02",
            "2026-07-02",
            "daily",
            1,
            2_000_000,
        );
        let r3 = insert_report_with_ts(
            &conn,
            &[p],
            "2026-07-03",
            "2026-07-03",
            "daily",
            1,
            3_000_000,
        );
        let items = list_report_history_impl(&conn, None, None, None).unwrap();
        assert_eq!(items.len(), 3);
        // 最后插入的(created_at 最大)应在第一位
        assert_eq!(items[0].id, r3);
        assert_eq!(items[2].id, r1);
        assert!(items[0].created_at >= items[1].created_at);
        assert!(items[1].created_at >= items[2].created_at);
        let _ = (r1, r2);
    }

    #[test]
    fn list_pagination_limit_offset() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        for _ in 0..5 {
            insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        }
        let page1 = list_report_history_impl(&conn, Some(2), Some(0), None).unwrap();
        let page2 = list_report_history_impl(&conn, Some(2), Some(2), None).unwrap();
        let page3 = list_report_history_impl(&conn, Some(2), Some(4), None).unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page3.len(), 1);
        // 三页 id 应互不重复
        let mut ids: Vec<i64> = page1
            .iter()
            .chain(page2.iter())
            .chain(page3.iter())
            .map(|i| i.id)
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn list_clamps_limit_to_200() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        let items = list_report_history_impl(&conn, Some(10_000), Some(0), None).unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn list_no_filter_returns_correct_names_and_counts() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        insert_report(&conn, &[a, b], "2026-07-01", "2026-07-01", "daily", 3);
        let items = list_report_history_impl(&conn, None, None, None).unwrap();
        assert_eq!(items.len(), 1);
        // 名称按 NOCASE 排序:alpha, beta
        assert_eq!(
            items[0].project_names,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        // total_commits = 3 + 3 = 6
        assert_eq!(items[0].total_commits, 6);
    }

    #[test]
    fn calendar_meta_groups_by_date_to() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        // 2026-07-01: 2 份日报
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        // 2026-07-05: 1 份周报
        insert_report(&conn, &[p], "2026-06-29", "2026-07-05", "weekly", 2);
        // 2026-07-10: 0 份,不应出现在 map 中
        // 2026-07-15: 1 份日报
        insert_report(&conn, &[p], "2026-07-15", "2026-07-15", "daily", 1);
        // 7 月范围:2026-07-01 ~ 2026-07-31
        let dates = get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &None).unwrap();
        assert_eq!(dates.get("2026-07-01").copied(), Some(2));
        assert_eq!(dates.get("2026-07-05").copied(), Some(1));
        assert_eq!(dates.get("2026-07-15").copied(), Some(1));
        assert!(!dates.contains_key("2026-07-10"));
    }

    #[test]
    fn calendar_meta_filters_by_project() {
        let conn = test_conn();
        let p1 = insert_project(&conn, "p1");
        let p2 = insert_project(&conn, "p2");
        insert_report(&conn, &[p1], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p2], "2026-07-01", "2026-07-01", "daily", 1);
        let only_p1 = get_calendar_meta_impl(&conn, 2026, 7, &[p1], &[], &None).unwrap();
        assert_eq!(only_p1.get("2026-07-01").copied(), Some(1));
        let only_p2 = get_calendar_meta_impl(&conn, 2026, 7, &[p2], &[], &None).unwrap();
        assert_eq!(only_p2.get("2026-07-01").copied(), Some(1));
        let both = get_calendar_meta_impl(&conn, 2026, 7, &[p1, p2], &[], &None).unwrap();
        assert_eq!(both.get("2026-07-01").copied(), Some(2));
    }

    #[test]
    fn calendar_meta_includes_neighbour_month_padding() {
        // 锁住"前后月填充日也能拿到报告计数"的行为:
        // reka-ui CalendarRoot 把上月末尾与下月开头作为填充日一起渲染,
        // 后端查询区间必须覆盖整张网格,否则填充格的标注丢失。
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        // 2026-07-01 是周三,网格首格 = 2026-06-29(周一)
        insert_report(&conn, &[p], "2026-06-29", "2026-06-29", "daily", 1);
        // 下个月首周内的日期(2026-08-04 周二)
        insert_report(&conn, &[p], "2026-08-04", "2026-08-04", "daily", 1);
        // 当月内对照点
        insert_report(&conn, &[p], "2026-07-15", "2026-07-15", "daily", 1);
        // 远离网格的日期(2026-08-10 已超过 grid_end = 2026-08-09)
        insert_report(&conn, &[p], "2026-08-10", "2026-08-10", "daily", 1);

        let dates = get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &None).unwrap();

        // 前月填充日
        assert_eq!(dates.get("2026-06-29").copied(), Some(1));
        // 当月内对照点
        assert_eq!(dates.get("2026-07-15").copied(), Some(1));
        // 下月填充日
        assert_eq!(dates.get("2026-08-04").copied(), Some(1));
        // 超出网格末尾的日期不应出现
        assert!(!dates.contains_key("2026-08-10"));
    }

    #[test]
    fn calendar_meta_filters_by_report_type() {
        let conn = test_conn();
        let p = insert_project(&conn, "p");
        insert_report(&conn, &[p], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[p], "2026-06-29", "2026-07-05", "weekly", 1);
        let only_daily =
            get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &Some("daily".into())).unwrap();
        assert_eq!(only_daily.get("2026-07-01").copied(), Some(1));
        assert!(!only_daily.contains_key("2026-07-05"));
        let only_weekly =
            get_calendar_meta_impl(&conn, 2026, 7, &[], &[], &Some("weekly".into())).unwrap();
        assert_eq!(only_weekly.get("2026-07-05").copied(), Some(1));
        assert!(!only_weekly.contains_key("2026-07-01"));
    }

    #[test]
    fn reports_by_date_returns_commits_and_aggregates() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        // 同一天两条报告,使用显式 created_at 以稳定断言排序
        let r1 = insert_report_with_ts(
            &conn,
            &[a],
            "2026-07-01",
            "2026-07-01",
            "daily",
            2,
            1_000_000,
        );
        let r2 = insert_report_with_ts(
            &conn,
            &[b],
            "2026-07-01",
            "2026-07-01",
            "daily",
            3,
            2_000_000,
        );

        let details =
            get_reports_by_range_impl(&conn, "2026-07-01", "2026-07-01", &[], &[], &None).unwrap();
        assert_eq!(details.len(), 2);

        // 顺序按 created_at DESC: r2 在前
        let first = &details[0];
        let second = &details[1];
        let (first_id, second_id) = (first.item.id, second.item.id);
        assert_eq!(first_id, r2);
        assert_eq!(second_id, r1);

        // first(r2): project_names=[beta], total_commits=3, commits.len()=3
        assert_eq!(first.item.project_names, vec!["beta".to_string()]);
        assert_eq!(first.item.total_commits, 3);
        assert_eq!(first.commits.len(), 1);
        assert_eq!(first.commits[0].commits.len(), 3);

        // second(r1): project_names=[alpha], total_commits=2
        assert_eq!(second.item.project_names, vec!["alpha".to_string()]);
        assert_eq!(second.item.total_commits, 2);
        assert_eq!(second.commits.len(), 1);
        assert_eq!(second.commits[0].commits.len(), 2);
    }

    #[test]
    fn reports_by_date_filters_by_project() {
        let conn = test_conn();
        let a = insert_project(&conn, "alpha");
        let b = insert_project(&conn, "beta");
        insert_report(&conn, &[a], "2026-07-01", "2026-07-01", "daily", 1);
        insert_report(&conn, &[b], "2026-07-01", "2026-07-01", "daily", 1);
        let only_a =
            get_reports_by_range_impl(&conn, "2026-07-01", "2026-07-01", &[a], &[], &None).unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].item.project_names, vec!["alpha".to_string()]);
    }

    #[test]
    fn batch_helpers_handle_empty_input() {
        let conn = test_conn();
        let names = resolve_project_names_batch(&conn, &[]).unwrap();
        assert!(names.is_empty());
        let counts = count_commits_batch(&conn, &[]).unwrap();
        assert!(counts.is_empty());
        let commits = load_report_commits_batch(&conn, &[]).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn list_report_history_empty_db() {
        let conn = test_conn();
        let items = list_report_history_impl(&conn, None, None, None).unwrap();
        assert!(items.is_empty());
        let items2 = list_report_history_impl(&conn, None, None, Some(1)).unwrap();
        assert!(items2.is_empty());
    }

    #[test]
    fn exact_project_id_filter_avoids_substring_matches() {
        let conn = test_conn();
        let now = crate::time_util::now_ts();
        for (pid, label) in [(1i64, "p1"), (12, "p12"), (123, "p123")] {
            conn.execute(
                "INSERT INTO projects (id, path, name, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, '', ?4, ?4)",
                params![pid, format!("/tmp/{label}-{now}"), label, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO report_history (project_ids, date_from, date_to, range_label,
                     author_mode, language, period_type, result, created_at)
                 VALUES (?1, '2026-07-01', '2026-07-01', '', 'me', 'zh-CN', 'daily', '', ?2)",
                params![format!("[{pid}]"), now],
            )
            .unwrap();
        }

        let exact = list_report_history_impl(&conn, None, None, Some(1)).unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].project_ids, vec![1]);
    }
}
