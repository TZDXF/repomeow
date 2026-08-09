use rusqlite::Connection;

use crate::error::AppResult;

const MIGRATION_001: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_favorite.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_pinned_commands.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_git_account_token_invalid.sql");

/// 按 PRAGMA user_version 顺序应用迁移,保证幂等
pub fn run(conn: &Connection) -> AppResult<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(MIGRATION_001)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        conn.execute_batch(MIGRATION_002)?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    if version < 3 {
        conn.execute_batch(MIGRATION_003)?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    if version < 4 {
        conn.execute_batch(MIGRATION_004)?;
        conn.pragma_update(None, "user_version", 4)?;
    }
    Ok(())
}
