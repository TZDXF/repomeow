use rusqlite::{params, Connection};
use tauri::{AppHandle, Emitter, State};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::PinnedCommand;

const KINDS: [&str; 4] = ["packageScript", "composeFile", "composeService", "customCommand"];

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// icon 不落库:customCommand 的 target_key 即命令 id,查询时实时 JOIN,
// 自定义命令改图标后无需重新标记即可生效
const PIN_COLS: &str =
    "p.id, p.project_id, p.kind, p.target_key, p.label, p.command, p.cwd, p.created_at, c.icon";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PinnedCommand> {
    Ok(PinnedCommand {
        id: r.get(0)?,
        project_id: r.get(1)?,
        kind: r.get(2)?,
        target_key: r.get(3)?,
        label: r.get(4)?,
        command: r.get(5)?,
        cwd: r.get(6)?,
        created_at: r.get(7)?,
        icon: r.get(8)?,
    })
}

/// 列出标记命令;project_id 为 None 时返回全部(托盘弹窗一次拉取)
pub fn list(conn: &Connection, project_id: Option<i64>) -> AppResult<Vec<PinnedCommand>> {
    let sql = format!(
        "SELECT {PIN_COLS} FROM pinned_commands p
         LEFT JOIN custom_commands c
           ON p.kind = 'customCommand' AND c.id = CAST(p.target_key AS INTEGER)
         {where} ORDER BY p.project_id, p.created_at, p.id",
        where = if project_id.is_some() {
            "WHERE p.project_id = ?1"
        } else {
            ""
        },
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = match project_id {
        Some(id) => stmt.query_map(params![id], map_row)?,
        None => stmt.query_map([], map_row)?,
    };
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[allow(clippy::too_many_arguments)]
pub fn set_pinned(
    conn: &Connection,
    project_id: i64,
    kind: &str,
    target_key: &str,
    pinned: bool,
    label: &str,
    command: &str,
    cwd: Option<&str>,
) -> AppResult<()> {
    if !KINDS.contains(&kind) {
        return Err(AppError::Invalid(format!("未知的标记类型: {kind}")));
    }
    if target_key.is_empty() {
        return Err(AppError::Invalid("标记项标识不能为空".into()));
    }
    if pinned {
        if label.is_empty() || command.is_empty() {
            return Err(AppError::Invalid("标记项名称与命令不能为空".into()));
        }
        // 已存在时刷新快照(命令文本可能已变化),幂等
        conn.execute(
            "INSERT INTO pinned_commands (project_id, kind, target_key, label, command, cwd, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (project_id, kind, target_key)
             DO UPDATE SET label = ?4, command = ?5, cwd = ?6",
            params![project_id, kind, target_key, label, command, cwd, now()],
        )?;
    } else {
        conn.execute(
            "DELETE FROM pinned_commands WHERE project_id = ?1 AND kind = ?2 AND target_key = ?3",
            params![project_id, kind, target_key],
        )?;
    }
    Ok(())
}

// ---- Tauri 命令包装 ----

#[tauri::command]
pub fn list_pinned_commands(
    db: State<'_, Db>,
    project_id: Option<i64>,
) -> AppResult<Vec<PinnedCommand>> {
    let conn = db.0.lock().unwrap();
    list(&conn, project_id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_pinned_command(
    app: AppHandle,
    db: State<'_, Db>,
    project_id: i64,
    kind: String,
    target_key: String,
    pinned: bool,
    label: Option<String>,
    command: Option<String>,
    cwd: Option<String>,
) -> AppResult<()> {
    {
        let conn = db.0.lock().unwrap();
        set_pinned(
            &conn,
            project_id,
            &kind,
            &target_key,
            pinned,
            label.as_deref().unwrap_or(""),
            command.as_deref().unwrap_or(""),
            cwd.as_deref(),
        )?;
    }
    // 托盘弹窗与主窗口是独立 Pinia 实例,广播标记变更让另一窗口同步刷新
    let _ = app.emit(
        "projects://pins-changed",
        serde_json::json!({ "projectId": project_id }),
    );
    Ok(())
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

    fn add_project(conn: &Connection) -> i64 {
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        project::add(conn, &dir, "demo", "").unwrap().id
    }

    #[test]
    fn set_list_unset() {
        let conn = test_conn();
        let pid = add_project(&conn);

        set_pinned(&conn, pid, "packageScript", ".\ndev", true, "dev", "npm run dev", None).unwrap();
        set_pinned(
            &conn,
            pid,
            "composeService",
            "compose.yml\ndb",
            true,
            "db",
            "docker compose -f \"compose.yml\"",
            None,
        )
        .unwrap();
        // 重复标记幂等,且刷新快照
        set_pinned(&conn, pid, "packageScript", ".\ndev", true, "dev", "npm run dev", None).unwrap();

        let items = list(&conn, None).unwrap();
        assert_eq!(items.len(), 2);
        let items = list(&conn, Some(pid)).unwrap();
        assert_eq!(items.len(), 2);
        assert!(list(&conn, Some(pid + 1)).unwrap().is_empty());

        set_pinned(&conn, pid, "packageScript", ".\ndev", false, "", "", None).unwrap();
        let items = list(&conn, Some(pid)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "composeService");
        // 取消不存在的标记不报错
        set_pinned(&conn, pid, "packageScript", ".\ndev", false, "", "", None).unwrap();
    }

    #[test]
    fn re_pin_refreshes_snapshot() {
        let conn = test_conn();
        let pid = add_project(&conn);

        set_pinned(&conn, pid, "packageScript", ".\ndev", true, "dev", "npm run dev", None).unwrap();
        // 同一 target 再次标记:不新增行,快照被刷新
        set_pinned(&conn, pid, "packageScript", ".\ndev", true, "dev-v2", "npm run dev2", Some("packages/web")).unwrap();

        let items = list(&conn, Some(pid)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "dev-v2");
        assert_eq!(items[0].command, "npm run dev2");
        assert_eq!(items[0].cwd.as_deref(), Some("packages/web"));
    }

    #[test]
    fn rejects_bad_input() {
        let conn = test_conn();
        let pid = add_project(&conn);

        assert!(matches!(
            set_pinned(&conn, pid, "bogus", "x", true, "x", "x", None),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            set_pinned(&conn, pid, "packageScript", "", true, "x", "x", None),
            Err(AppError::Invalid(_))
        ));
        assert!(matches!(
            set_pinned(&conn, pid, "packageScript", "x", true, "", "x", None),
            Err(AppError::Invalid(_))
        ));
    }

    #[test]
    fn custom_command_pin_carries_icon() {
        use crate::commands::script;
        let conn = test_conn();
        let pid = add_project(&conn);

        let cmd = script::create_command(&conn, pid, "部署", "make deploy", "", "rocket").unwrap();
        set_pinned(
            &conn,
            pid,
            "customCommand",
            &cmd.id.to_string(),
            true,
            "部署",
            "make deploy",
            None,
        )
        .unwrap();
        set_pinned(&conn, pid, "packageScript", ".\ndev", true, "dev", "npm run dev", None).unwrap();

        let items = list(&conn, Some(pid)).unwrap();
        let custom = items.iter().find(|p| p.kind == "customCommand").unwrap();
        assert_eq!(custom.icon.as_deref(), Some("rocket"));
        // 非自定义命令没有图标
        let npm = items.iter().find(|p| p.kind == "packageScript").unwrap();
        assert_eq!(npm.icon, None);
    }

    #[test]
    fn cascades_on_project_delete() {
        let conn = test_conn();
        let pid = add_project(&conn);
        set_pinned(&conn, pid, "composeFile", "compose.yml", true, "compose.yml", "docker compose -f \"compose.yml\"", None).unwrap();

        conn.execute("DELETE FROM projects WHERE id = ?1", params![pid])
            .unwrap();
        assert!(list(&conn, Some(pid)).unwrap().is_empty());
    }
}
