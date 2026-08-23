use rusqlite::{params, Connection};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::Tag;

const DEFAULT_COLOR: &str = "#3b82f6";

fn validate_color(color: &str) -> AppResult<String> {
    let color = color.trim();
    if color.is_empty() {
        return Ok(DEFAULT_COLOR.to_string());
    }
    let valid = color.starts_with('#')
        && (4..=9).contains(&color.len())
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if valid {
        Ok(color.to_string())
    } else {
        Err(AppError::coded(
            ErrorCode::TagColorInvalid,
            color.to_string(),
        ))
    }
}

pub fn all(conn: &Connection) -> AppResult<Vec<Tag>> {
    let mut stmt = conn.prepare("SELECT id, name, color FROM tags ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], |r| {
        Ok(Tag {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn create(conn: &Connection, name: &str, color: &str) -> AppResult<Tag> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::coded(ErrorCode::TagNameRequired, ""));
    }
    let color = validate_color(color)?;
    conn.execute(
        "INSERT INTO tags (name, color) VALUES (?1, ?2)",
        params![name, color],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            AppError::coded(ErrorCode::TagNameConflict, name.to_string())
        }
        other => AppError::Db(other),
    })?;
    Ok(Tag {
        id: conn.last_insert_rowid(),
        name: name.to_string(),
        color,
    })
}

pub fn remove(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::TagNotFound, id.to_string()));
    }
    Ok(())
}

pub fn update(conn: &Connection, id: i64, name: &str, color: &str) -> AppResult<Tag> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::coded(ErrorCode::TagNameRequired, ""));
    }
    let color = validate_color(color)?;
    let changed = conn
        .execute(
            "UPDATE tags SET name = ?1, color = ?2 WHERE id = ?3",
            params![name, color, id],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                AppError::coded(ErrorCode::TagNameConflict, name.to_string())
            }
            other => AppError::Db(other),
        })?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::TagNotFound, id.to_string()));
    }
    Ok(Tag {
        id,
        name: name.to_string(),
        color,
    })
}

pub fn apply_project_tags(conn: &Connection, project_id: i64, tag_ids: &[i64]) -> AppResult<()> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1",
        params![project_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(AppError::coded(
            ErrorCode::ProjectNotFound,
            project_id.to_string(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM project_tags WHERE project_id = ?1",
        params![project_id],
    )?;
    for tag_id in tag_ids {
        // 外键校验 tag 存在;重复 id 忽略
        tx.execute(
            "INSERT OR IGNORE INTO project_tags (project_id, tag_id) VALUES (?1, ?2)",
            params![project_id, tag_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// ---- Tauri 命令包装 ----

#[tauri::command]
pub fn list_tags(db: State<'_, Db>) -> AppResult<Vec<Tag>> {
    let conn = db.0.lock().unwrap();
    all(&conn)
}

#[tauri::command]
pub fn create_tag(db: State<'_, Db>, name: String, color: String) -> AppResult<Tag> {
    let conn = db.0.lock().unwrap();
    create(&conn, &name, &color)
}

#[tauri::command]
pub fn delete_tag(db: State<'_, Db>, id: i64) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    remove(&conn, id)
}

#[tauri::command]
pub fn update_tag(db: State<'_, Db>, id: i64, name: String, color: String) -> AppResult<Tag> {
    let conn = db.0.lock().unwrap();
    update(&conn, id, &name, &color)
}

#[tauri::command]
pub fn set_project_tags(db: State<'_, Db>, project_id: i64, tag_ids: Vec<i64>) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    apply_project_tags(&conn, project_id, &tag_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::project;
    use crate::db;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    #[test]
    fn create_list_delete() {
        let conn = test_conn();
        let t = create(&conn, "work", "").unwrap();
        assert_eq!(t.color, DEFAULT_COLOR);
        let t2 = create(&conn, "oss", "#22c55e").unwrap();

        let tags = all(&conn).unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[1].name, "work"); // name 排序: oss < work

        remove(&conn, t2.id).unwrap();
        assert_eq!(all(&conn).unwrap().len(), 1);
        assert!(matches!(remove(&conn, t2.id), Err(ref e) if e.is_code(ErrorCode::TagNotFound)));
        let _ = t;
    }

    #[test]
    fn duplicate_name_conflicts() {
        let conn = test_conn();
        create(&conn, "work", "").unwrap();
        assert!(matches!(
            create(&conn, "work", "#fff"),
            Err(ref e) if e.is_code(ErrorCode::TagNameConflict)
        ));
    }

    #[test]
    fn update_renames_and_recolors() {
        let conn = test_conn();
        let t = create(&conn, "work", "").unwrap();

        let t2 = update(&conn, t.id, "job", "#22c55e").unwrap();
        assert_eq!(t2.id, t.id);
        assert_eq!(t2.name, "job");
        assert_eq!(t2.color, "#22c55e");
        assert_eq!(all(&conn).unwrap()[0].name, "job");

        // 不存在的 tag
        assert!(matches!(
            update(&conn, 9999, "x", ""),
            Err(ref e) if e.is_code(ErrorCode::TagNotFound)
        ));
        // 空名称 / 非法颜色
        assert!(matches!(
            update(&conn, t.id, " ", ""),
            Err(ref e) if e.is_code(ErrorCode::TagNameRequired)
        ));
        assert!(matches!(
            update(&conn, t.id, "x", "red"),
            Err(ref e) if e.is_code(ErrorCode::TagColorInvalid)
        ));
        // 重名冲突
        create(&conn, "other", "").unwrap();
        assert!(matches!(
            update(&conn, t.id, "other", ""),
            Err(ref e) if e.is_code(ErrorCode::TagNameConflict)
        ));
    }

    #[test]
    fn rejects_bad_input() {
        let conn = test_conn();
        assert!(
            matches!(create(&conn, " ", ""), Err(ref e) if e.is_code(ErrorCode::TagNameRequired))
        );
        assert!(matches!(
            create(&conn, "x", "not-a-color"),
            Err(ref e) if e.is_code(ErrorCode::TagColorInvalid)
        ));
        assert!(create(&conn, "x", "#a1b2").is_ok());
    }

    #[test]
    fn set_project_tags_replaces_and_cascades() {
        let conn = test_conn();
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let p = project::add(&conn, &dir, "demo", "").unwrap();
        let t1 = create(&conn, "a", "").unwrap();
        let t2 = create(&conn, "b", "").unwrap();

        apply_project_tags(&conn, p.id, &[t1.id, t2.id]).unwrap();
        assert_eq!(project::load_tags(&conn, p.id).unwrap().len(), 2);

        // 覆盖式设置
        apply_project_tags(&conn, p.id, &[t2.id]).unwrap();
        let tags = project::load_tags(&conn, p.id).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "b");

        // 不存在的 tag 违反外键
        assert!(apply_project_tags(&conn, p.id, &[9999]).is_err());
        // 项目不存在
        assert!(matches!(
            apply_project_tags(&conn, 9999, &[]),
            Err(ref e) if e.is_code(ErrorCode::ProjectNotFound)
        ));

        // 删除 tag 级联清理关联
        apply_project_tags(&conn, p.id, &[t1.id]).unwrap();
        remove(&conn, t1.id).unwrap();
        assert!(project::load_tags(&conn, p.id).unwrap().is_empty());
    }
}
