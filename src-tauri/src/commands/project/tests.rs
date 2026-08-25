use rusqlite::{params, Connection};

use super::*;
use crate::db;

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init(&conn).unwrap();
    conn
}

#[test]
fn archive_hides_from_list_but_keeps_data() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();

    let p = add(&conn, &dir, "demo", "").unwrap();
    assert_eq!(p.name, "demo");
    assert!(p.tags.is_empty());
    assert!(p.git.is_none());
    assert!(p.archived_at.is_none());

    let fetched = get(&conn, p.id).unwrap();
    assert_eq!(fetched.path, p.path);

    let all = list(&conn, None, None).unwrap();
    assert_eq!(all.len(), 1);

    archive(&conn, p.id).unwrap();
    // 归档后不再出现在列表中,但数据保留(get 仍可取到)
    assert!(list(&conn, None, None).unwrap().is_empty());
    let archived = get(&conn, p.id).unwrap();
    assert!(archived.archived_at.is_some());

    assert!(
        matches!(archive(&conn, 9999), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn set_favorite_toggles_favorited_at() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let p = add(&conn, &dir, "demo", "").unwrap();
    assert!(p.favorited_at.is_none());

    set_favorite(&conn, p.id, true).unwrap();
    assert!(get(&conn, p.id).unwrap().favorited_at.is_some());

    set_favorite(&conn, p.id, false).unwrap();
    assert!(get(&conn, p.id).unwrap().favorited_at.is_none());

    assert!(
        matches!(set_favorite(&conn, 9999, true), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn set_auto_pull_toggles_flag() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let p = add(&conn, &dir, "demo", "").unwrap();
    assert!(!p.auto_pull);

    set_auto_pull(&conn, p.id, true).unwrap();
    assert!(get(&conn, p.id).unwrap().auto_pull);

    set_auto_pull(&conn, p.id, false).unwrap();
    assert!(!get(&conn, p.id).unwrap().auto_pull);

    assert!(
        matches!(set_auto_pull(&conn, 9999, true), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn set_wiki_auto_update_toggles_flag() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let p = add(&conn, &dir, "demo", "").unwrap();
    // 默认关闭,且与 auto_pull 互相独立
    assert!(!p.wiki_auto_update);

    set_wiki_auto_update(&conn, p.id, true).unwrap();
    assert!(get(&conn, p.id).unwrap().wiki_auto_update);
    assert!(!get(&conn, p.id).unwrap().auto_pull);

    set_wiki_auto_update(&conn, p.id, false).unwrap();
    assert!(!get(&conn, p.id).unwrap().wiki_auto_update);

    assert!(
        matches!(set_wiki_auto_update(&conn, 9999, true), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn unarchive_restores_to_list() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let p = add(&conn, &dir, "demo", "").unwrap();
    archive(&conn, p.id).unwrap();

    // 归档列表按归档时间倒序返回
    let archived = list_archived(&conn).unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, p.id);
    assert!(archived[0].archived_at.is_some());

    unarchive(&conn, p.id).unwrap();
    assert!(list_archived(&conn).unwrap().is_empty());
    assert_eq!(list(&conn, None, None).unwrap().len(), 1);
    assert!(get(&conn, p.id).unwrap().archived_at.is_none());

    // 未归档 / 不存在的项目
    assert!(
        matches!(unarchive(&conn, p.id), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
    assert!(
        matches!(unarchive(&conn, 9999), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn remove_deletes_permanently() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let p = add(&conn, &dir, "demo", "").unwrap();
    archive(&conn, p.id).unwrap();

    remove(&conn, p.id).unwrap();
    assert!(
        matches!(get(&conn, p.id), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
    assert!(list_archived(&conn).unwrap().is_empty());
    assert!(
        matches!(remove(&conn, p.id), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn duplicate_path_conflicts() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    add(&conn, &dir, "a", "").unwrap();
    assert!(matches!(
        add(&conn, &dir, "b", ""),
        Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectPathConflict)
    ));
}

#[test]
fn add_normalizes_path_style_before_insert() {
    let conn = test_conn();
    let dir = std::env::temp_dir();
    // 正斜杠 + 尾斜杠写法登记,库里存的是归一化形态
    let styled = format!("{}/", crate::path_util::to_forward_slash(&dir));
    let p = add(&conn, &styled, "a", "").unwrap();
    assert_eq!(p.path, crate::path_util::clean_str(&dir.to_string_lossy()));
    // 同一目录换原生分隔符写法再登记 → 冲突,不会重复登记成两个项目
    assert!(matches!(
        add(&conn, &dir.to_string_lossy(), "b", ""),
        Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectPathConflict)
    ));
}

#[test]
fn normalize_stored_paths_cleans_legacy_rows() {
    let conn = test_conn();
    let dir = std::env::temp_dir();
    // 模拟归一化之前的历史数据:正斜杠 + 尾斜杠
    let legacy = format!("{}/", crate::path_util::to_forward_slash(&dir));
    conn.execute(
        "INSERT INTO projects (path, name, description, created_at, updated_at)
         VALUES (?1, 'legacy', '', 0, 0)",
        params![legacy],
    )
    .unwrap();
    let changed = normalize_stored_paths(&conn);
    assert_eq!(changed, 1);
    // 再跑幂等
    assert_eq!(normalize_stored_paths(&conn), 0);
    let stored: String = conn
        .query_row("SELECT path FROM projects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, crate::path_util::clean_str(&legacy));
}

#[test]
fn rejects_bad_input() {
    let conn = test_conn();
    assert!(matches!(add(&conn, "C:/definitely/not/exist", "x", ""),
            Err(ref e) if e.is_code(crate::error::ErrorCode::InvalidPath)));
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    assert!(matches!(
        add(&conn, &dir, "   ", ""),
        Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNameRequired)
    ));
}

#[test]
fn update_changes_fields() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let p = add(&conn, &dir, "old", "").unwrap();
    let p2 = update(&conn, p.id, "new", "desc").unwrap();
    assert_eq!(p2.name, "new");
    assert_eq!(p2.description, "desc");
    assert!(p2.updated_at >= p.updated_at);
    assert!(
        matches!(update(&conn, 9999, "x", ""), Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
}

#[test]
fn update_path_relocates_and_validates() {
    let conn = test_conn();
    let dir = std::env::temp_dir();
    let a_path = dir.join("repomeow-relocate-a");
    let b_path = dir.join("repomeow-relocate-b");
    std::fs::create_dir_all(&a_path).unwrap();
    std::fs::create_dir_all(&b_path).unwrap();
    let a = add(&conn, &a_path.to_string_lossy(), "a", "").unwrap();
    let b = add(&conn, &b_path.to_string_lossy(), "b", "").unwrap();

    // 不存在的目录 / 已被其他项目登记的目录都拒绝
    assert!(
        matches!(update_path(&conn, a.id, "C:/definitely/not/exist"),
            Err(ref e) if e.is_code(crate::error::ErrorCode::InvalidPath))
    );
    assert!(matches!(
        update_path(&conn, a.id, &b.path),
        Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectPathConflict)
    ));

    // 换到一个新目录:path 更新,path_exists 重新计算
    let new_path = dir.join("repomeow-relocate-c");
    std::fs::create_dir_all(&new_path).unwrap();
    let moved = update_path(&conn, a.id, &new_path.to_string_lossy()).unwrap();
    assert_eq!(moved.path, new_path.to_string_lossy());
    assert!(moved.path_exists);
    assert!(moved.updated_at >= a.updated_at);

    assert!(
        matches!(update_path(&conn, 9999, &new_path.to_string_lossy()),
            Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectNotFound))
    );
    drop(b);
}

#[test]
fn move_dir_renames_and_validates() {
    let conn = test_conn();
    let dir = std::env::temp_dir();
    let src = dir.join("repomeow-move-src");
    let other = dir.join("repomeow-move-other");
    let taken = dir.join("repomeow-move-taken");
    let dst = dir.join("repomeow-move-dst");
    // 清理上一轮测试残留,保证可重复运行
    std::fs::remove_dir_all(&dst).ok();
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::create_dir_all(&taken).unwrap();
    let p = add(&conn, &src.to_string_lossy(), "demo", "").unwrap();
    let _other_p = add(&conn, &other.to_string_lossy(), "other", "").unwrap();

    // 目标已存在 / 已被其他项目登记 / 移入自身内部 / 位置未变化 / 目录名带分隔符,均拒绝
    assert!(matches!(
        move_dir(&conn, p.id, &dir.to_string_lossy(), "repomeow-move-taken"),
        Err(ref e) if e.is_code(crate::error::ErrorCode::MoveTargetExists)
    ));
    // 目标路径已被其他项目登记(磁盘目录已存在 → MoveTargetExists 优先)
    // 验证路径冲突的 ProjectPathConflict:把 other 目录移除后再试
    std::fs::remove_dir_all(&other).unwrap();
    assert!(matches!(
        move_dir(&conn, p.id, &dir.to_string_lossy(), "repomeow-move-other"),
        Err(ref e) if e.is_code(crate::error::ErrorCode::ProjectPathConflict)
    ));
    // 还原以供后续不受影响
    std::fs::create_dir_all(&other).unwrap();
    assert!(matches!(
        move_dir(&conn, p.id, &src.to_string_lossy(), "inner"),
        Err(ref e) if e.is_code(crate::error::ErrorCode::MoveInsideSelf)
    ));
    assert!(matches!(
        move_dir(&conn, p.id, &dir.to_string_lossy(), "repomeow-move-src"),
        Err(ref e) if e.is_code(crate::error::ErrorCode::MoveSameLocation)
    ));
    assert!(matches!(
        move_dir(&conn, p.id, &dir.to_string_lossy(), "bad/name"),
        Err(ref e) if e.is_code(crate::error::ErrorCode::MoveInvalidDirName)
    ));

    // 正常移动 + 改名:磁盘目录移动,登记路径同步更新
    let moved = move_dir(&conn, p.id, &dir.to_string_lossy(), "repomeow-move-dst").unwrap();
    assert!(!src.exists() && dst.is_dir());
    assert_eq!(moved.path, dst.to_string_lossy());
    assert!(moved.path_exists);

    std::fs::remove_dir_all(&dst).ok();
    std::fs::remove_dir_all(&taken).ok();
}

#[test]
fn list_loads_tags_in_project_order_and_keeps_empty_projects() {
    let conn = test_conn();
    let dir = std::env::temp_dir();
    let a_path = dir.join("repomeow-batch-a");
    let b_path = dir.join("repomeow-batch-b");
    std::fs::create_dir_all(&a_path).unwrap();
    std::fs::create_dir_all(&b_path).unwrap();
    let a = add(&conn, &a_path.to_string_lossy(), "Alpha", "").unwrap();
    let b = add(&conn, &b_path.to_string_lossy(), "Beta", "").unwrap();
    conn.execute("INSERT INTO tags (name, color) VALUES ('zeta', '#z')", [])
        .unwrap();
    let zeta = conn.last_insert_rowid();
    conn.execute("INSERT INTO tags (name, color) VALUES ('alpha', '#a')", [])
        .unwrap();
    let alpha = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO project_tags (project_id, tag_id) VALUES (?1, ?2), (?1, ?3)",
        params![a.id, zeta, alpha],
    )
    .unwrap();

    let projects = list(&conn, None, None).unwrap();
    assert_eq!(
        projects.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![a.id, b.id]
    );
    assert_eq!(
        projects[0]
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    assert!(projects[1].tags.is_empty());
}

#[test]
fn list_filters_by_name_and_tags() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let dir_b = std::env::temp_dir().join("repomeow-test-beta");
    std::fs::create_dir_all(&dir_b).unwrap();
    let dir_b = dir_b.to_string_lossy().to_string();
    let a = add(&conn, &dir, "Alpha", "").unwrap();
    let _b = add(&conn, &dir_b, "Beta", "").unwrap();

    let hit = list(&conn, Some("alph".into()), None).unwrap();
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].name, "Alpha");

    // 直接造标签数据验证 tag_ids 过滤
    conn.execute("INSERT INTO tags (name, color) VALUES ('work', '#fff')", [])
        .unwrap();
    let tag_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO project_tags (project_id, tag_id) VALUES (?1, ?2)",
        params![a.id, tag_id],
    )
    .unwrap();

    let filtered = list(&conn, None, Some(vec![tag_id])).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].tags.len(), 1);
    assert_eq!(filtered[0].tags[0].name, "work");

    let empty = list(&conn, None, Some(vec![tag_id + 100])).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn list_query_splits_space_separated_terms_with_and() {
    let conn = test_conn();
    let dir = std::env::temp_dir().to_string_lossy().to_string();
    let dir_b = std::env::temp_dir().join("repomeow-test-beta");
    std::fs::create_dir_all(&dir_b).unwrap();
    let dir_b = dir_b.to_string_lossy().to_string();
    add(&conn, &dir, "Alpha", "web 前端").unwrap();
    add(&conn, &dir_b, "Beta", "web 后端").unwrap();

    // 两词分别命中名称与描述:AND 后只剩 Alpha
    let hit = list(&conn, Some("alpha web".into()), None).unwrap();
    assert_eq!(hit.len(), 1);
    assert_eq!(hit[0].name, "Alpha");

    // 任一词不命中即无结果;多余空白不影响切分
    assert!(list(&conn, Some("web  alpha   beta ".into()), None)
        .unwrap()
        .is_empty());
}
