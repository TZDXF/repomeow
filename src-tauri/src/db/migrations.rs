use rusqlite::Connection;

use crate::error::AppResult;

const MIGRATION_001: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_favorite.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_pinned_commands.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_git_account_token_invalid.sql");
const MIGRATION_005: &str = include_str!("../../migrations/005_auto_pull.sql");
const MIGRATION_006: &str = include_str!("../../migrations/006_schedule_tag_ids.sql");
const MIGRATION_007: &str = include_str!("../../migrations/007_daily_previous_day.sql");
const MIGRATION_008: &str = include_str!("../../migrations/008_wiki_auto_update.sql");
const MIGRATION_009: &str = include_str!("../../migrations/009_ai_usage_log.sql");
const MIGRATION_010: &str = include_str!("../../migrations/010_ai_usage_cached_tokens.sql");

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
    if version < 5 {
        conn.execute_batch(MIGRATION_005)?;
        conn.pragma_update(None, "user_version", 5)?;
    }
    if version < 6 {
        conn.execute_batch(MIGRATION_006)?;
        conn.pragma_update(None, "user_version", 6)?;
    }
    if version < 7 {
        conn.execute_batch(MIGRATION_007)?;
        conn.pragma_update(None, "user_version", 7)?;
    }
    if version < 8 {
        conn.execute_batch(MIGRATION_008)?;
        conn.pragma_update(None, "user_version", 8)?;
    }
    if version < 9 {
        conn.execute_batch(MIGRATION_009)?;
        conn.pragma_update(None, "user_version", 9)?;
    }
    if version < 10 {
        conn.execute_batch(MIGRATION_010)?;
        conn.pragma_update(None, "user_version", 10)?;
    }
    Ok(())
}
