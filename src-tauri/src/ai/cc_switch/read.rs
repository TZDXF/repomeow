use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::AppResult;
use crate::time_util::now_ts_nanos;

use super::parse::text;
use super::*;

// ── 读取(SQLite 数据库 / 旧版 config.json) ─────────────────────────

/// 数据库行(只取解析所需的核心列,旧库缺新增列也能查)。
pub(super) struct RawProvider {
    pub(super) id: String,
    pub(super) app: String,
    pub(super) name: String,
    pub(super) settings_config: Value,
    pub(super) current: bool,
}

/// CC Switch 可能正在运行并持有数据库;复制(含 WAL 侧车文件)到临时目录再打开,
/// 避免锁冲突,也能读到 WAL 中已提交的最新数据。回调拿到的是临时副本路径。
pub(super) fn with_staged_db<T>(db_path: &Path, f: impl FnOnce(&Path) -> AppResult<T>) -> AppResult<T> {
    let staging = std::env::temp_dir().join(format!("repomeow-cc-switch-{}", now_ts_nanos()));
    fs::create_dir_all(&staging)?;
    let result = (|| {
        let copy = staging.join("cc-switch.db");
        fs::copy(db_path, &copy)?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
            if sidecar.is_file() {
                fs::copy(&sidecar, staging.join(format!("cc-switch.db{suffix}")))?;
            }
        }
        f(&copy)
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

pub(super) fn read_providers_db(db_path: &Path) -> AppResult<Vec<RawProvider>> {
    with_staged_db(db_path, |copy| query_providers(copy))
}

fn query_providers(path: &Path) -> AppResult<Vec<RawProvider>> {
    let conn = Connection::open(path)?;
    let mut stmt = match conn
        .prepare("SELECT id, app_type, name, settings_config, is_current FROM providers")
    {
        Ok(stmt) => stmt,
        // 表不存在(空库/未知版本)按无供应商处理
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.contains("no such table") =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error.into()),
    };
    let rows = stmt.query_map([], |row| {
        let config_text: String = row.get(3)?;
        Ok(RawProvider {
            id: row.get(0)?,
            app: row.get(1)?,
            name: row.get(2)?,
            settings_config: serde_json::from_str(&config_text).unwrap_or(Value::Null),
            current: row.get::<_, i64>(4).unwrap_or(0) != 0,
        })
    })?;
    let mut providers = Vec::new();
    for row in rows {
        providers.push(row?);
    }
    Ok(providers)
}

/// skills / mcp_servers 表的 enabled_* 列 → 应用 id。
const ENABLED_APP_COLUMNS: &[(&str, &str)] = &[
    ("enabled_claude", "claude"),
    ("enabled_codex", "codex"),
    ("enabled_gemini", "gemini"),
    ("enabled_grokbuild", "grokbuild"),
    ("enabled_opencode", "opencode"),
    ("enabled_hermes", "hermes"),
];

fn enabled_apps(row: &rusqlite::Row<'_>, base: usize) -> Vec<String> {
    ENABLED_APP_COLUMNS
        .iter()
        .enumerate()
        .filter_map(|(i, (_, app))| {
            (row.get::<_, i64>(base + i).unwrap_or(0) != 0).then(|| app.to_string())
        })
        .collect()
}

/// 缺表(旧版库)按空列表处理;缺列等其他错误上抛。
fn table_missing(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("no such table")
    )
}

pub(super) fn query_skills(conn: &Connection) -> AppResult<Vec<CcSwitchSkill>> {
    let mut stmt = match conn.prepare(
        "SELECT id, name, description, directory, \
         enabled_claude, enabled_codex, enabled_gemini, \
         enabled_grokbuild, enabled_opencode, enabled_hermes FROM skills",
    ) {
        Ok(stmt) => stmt,
        Err(error) if table_missing(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let rows = stmt.query_map([], |row| {
        Ok(CcSwitchSkill {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            directory: row.get(3)?,
            enabled_apps: enabled_apps(row, 4),
        })
    })?;
    let mut skills = Vec::new();
    for row in rows {
        skills.push(row?);
    }
    Ok(skills)
}

pub(super) fn query_mcp_servers(conn: &Connection) -> AppResult<Vec<CcSwitchMcpServer>> {
    let mut stmt = match conn.prepare(
        "SELECT id, name, server_config, description, tags, \
         enabled_claude, enabled_codex, enabled_gemini, \
         enabled_grokbuild, enabled_opencode, enabled_hermes FROM mcp_servers",
    ) {
        Ok(stmt) => stmt,
        Err(error) if table_missing(&error) => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let rows = stmt.query_map([], |row| {
        let config_text: String = row.get(2)?;
        let tags_text: String = row.get::<_, Option<String>>(4)?.unwrap_or_default();
        Ok(CcSwitchMcpServer {
            id: row.get(0)?,
            name: row.get(1)?,
            server_config: serde_json::from_str(&config_text).unwrap_or(Value::Null),
            description: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            tags: serde_json::from_str(&tags_text).unwrap_or_default(),
            enabled_apps: enabled_apps(row, 5),
        })
    })?;
    let mut servers = Vec::new();
    for row in rows {
        servers.push(row?);
    }
    Ok(servers)
}

/// 旧版 CC Switch(<3.x)的 `config.json`:`{ apps: { <app>: { providers: {id: Provider}, current } } }`。
/// 解析失败按无供应商处理(不阻断,数据库才是新版事实源)。
pub(super) fn read_legacy_config(path: &Path) -> Vec<RawProvider> {    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(apps) = value.get("apps").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut providers = Vec::new();
    for (app, manager) in apps {
        let current = manager.get("current").and_then(Value::as_str).unwrap_or("");
        let Some(entries) = manager.get("providers").and_then(Value::as_object) else {
            continue;
        };
        for (id, provider) in entries {
            let Some(settings_config) = provider.get("settingsConfig").cloned() else {
                continue;
            };
            providers.push(RawProvider {
                id: id.clone(),
                app: app.clone(),
                name: text(provider.get("name")),
                settings_config,
                current: current == id,
            });
        }
    }
    providers
}
