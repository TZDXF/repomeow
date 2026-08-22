pub mod migrations;

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::AppResult;

/// 全局共享的 SQLite 连接(rusqlite Connection 非 Sync,用 Mutex 保护)
pub struct Db(pub Mutex<Connection>);

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        init(&conn)?;
        Ok(Self(Mutex::new(conn)))
    }
}

pub fn init(conn: &Connection) -> AppResult<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    migrations::run(conn)?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    use rusqlite::OptionalExtension;
    let value = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(value)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_db_file_and_tables() {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-test-{}-{}",
            std::process::id(),
            crate::time_util::now_ts_nanos()
        ));
        let db_path = dir.join("projects.db");
        let db = Db::open(&db_path).expect("open db");
        assert!(db_path.exists(), "DB 文件应被创建");

        let conn = db.0.lock().unwrap();
        for table in [
            "projects",
            "tags",
            "project_tags",
            "custom_commands",
            "settings",
            "hidden_items",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "表 {table} 应存在");
        }
        // 迁移幂等:重复执行不报错
        migrations::run(&conn).expect("migrations idempotent");

        drop(conn);
        drop(db);
        let _ = fs::remove_dir_all(&dir);
    }
}
