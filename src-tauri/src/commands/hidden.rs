use rusqlite::{params, Connection};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::HiddenItem;

const KINDS: [&str; 3] = ["packageFile", "packageScript", "composeFile"];

pub fn list(conn: &Connection, project_id: i64) -> AppResult<Vec<HiddenItem>> {
    let mut stmt = conn.prepare(
        "SELECT kind, target_key FROM hidden_items WHERE project_id = ?1 ORDER BY kind, target_key",
    )?;
    let rows = stmt.query_map(params![project_id], |r| {
        Ok(HiddenItem {
            kind: r.get(0)?,
            target_key: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn set_hidden(
    conn: &Connection,
    project_id: i64,
    kind: &str,
    target_key: &str,
    hidden: bool,
) -> AppResult<()> {
    if !KINDS.contains(&kind) {
        return Err(AppError::coded(ErrorCode::HiddenItemTypeUnknown, kind.to_string()));
    }
    if target_key.is_empty() {
        return Err(AppError::coded(ErrorCode::HiddenItemKeyRequired, ""));
    }
    if hidden {
        conn.execute(
            "INSERT OR IGNORE INTO hidden_items (project_id, kind, target_key) VALUES (?1, ?2, ?3)",
            params![project_id, kind, target_key],
        )?;
    } else {
        conn.execute(
            "DELETE FROM hidden_items WHERE project_id = ?1 AND kind = ?2 AND target_key = ?3",
            params![project_id, kind, target_key],
        )?;
    }
    Ok(())
}

// ---- Tauri 命令包装 ----
// (列表查询已并入 commands::overview::get_project_overview,详情页一次 IPC 取全)

#[tauri::command]
pub fn set_hidden_item(
    db: State<'_, Db>,
    project_id: i64,
    kind: String,
    target_key: String,
    hidden: bool,
) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    set_hidden(&conn, project_id, &kind, &target_key, hidden)
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
    fn set_list_unset() {
        let conn = test_conn();
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let p = project::add(&conn, &dir, "demo", "").unwrap();

        set_hidden(&conn, p.id, "packageFile", ".", true).unwrap();
        set_hidden(&conn, p.id, "packageScript", ".\ndev", true).unwrap();
        // 重复隐藏幂等
        set_hidden(&conn, p.id, "packageFile", ".", true).unwrap();

        let items = list(&conn, p.id).unwrap();
        assert_eq!(items.len(), 2);

        set_hidden(&conn, p.id, "packageFile", ".", false).unwrap();
        let items = list(&conn, p.id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "packageScript");
        // 删除不存在的项不报错
        set_hidden(&conn, p.id, "packageFile", ".", false).unwrap();
    }

    #[test]
    fn rejects_bad_input() {
        let conn = test_conn();
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let p = project::add(&conn, &dir, "demo", "").unwrap();

        assert!(matches!(
            set_hidden(&conn, p.id, "bogus", "x", true),
            Err(ref e) if e.is_code(ErrorCode::HiddenItemTypeUnknown)
        ));
        assert!(matches!(
            set_hidden(&conn, p.id, "packageFile", "", true),
            Err(ref e) if e.is_code(ErrorCode::HiddenItemKeyRequired)
        ));
    }

    #[test]
    fn cascades_on_project_delete() {
        let conn = test_conn();
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let p = project::add(&conn, &dir, "demo", "").unwrap();
        set_hidden(&conn, p.id, "composeFile", "compose.yml", true).unwrap();

        conn.execute("DELETE FROM projects WHERE id = ?1", params![p.id])
            .unwrap();
        assert!(list(&conn, p.id).unwrap().is_empty());
    }
}
