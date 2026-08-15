use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Emitter, State};

use crate::commands::open::spawn_terminal;
use crate::commands::walk;
use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{CustomCommand, PackageScript, PackageScriptsGroup};

/// 解析 package.json 内容,返回 (包名, scripts)。
/// 解析失败、无 scripts 字段或 scripts 为空时返回 None。
fn parse_package_json(content: &str) -> Option<(Option<String>, Vec<PackageScript>)> {
    let json = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let scripts: Vec<PackageScript> = json
        .get("scripts")?
        .as_object()?
        .iter()
        .filter_map(|(name, cmd)| {
            cmd.as_str().map(|c| PackageScript {
                name: name.clone(),
                command: c.to_string(),
            })
        })
        .collect();
    if scripts.is_empty() {
        return None;
    }
    let package_name = json.get("name").and_then(|n| n.as_str()).map(String::from);
    Some((package_name, scripts))
}

/// 递归发现项目内所有 package.json(尊重 git 排除规则),按包分组返回 scripts。
/// 支持 monorepo;node_modules 恒被跳过。
/// 前端走合并扫描 scan_project_assets,此入口保留给测试
#[cfg_attr(not(test), allow(dead_code))]
pub fn package_scripts(path: &str) -> AppResult<Vec<PackageScriptsGroup>> {
    let dir = std::path::Path::new(path);
    Ok(package_scripts_from_files(dir, &walk::project_files(dir)))
}

/// 在已遍历的文件清单上提取 package scripts(供合并扫描复用,避免重复 walk)
pub(crate) fn package_scripts_from_files(
    dir: &std::path::Path,
    files: &[std::path::PathBuf],
) -> Vec<PackageScriptsGroup> {
    let mut groups: Vec<PackageScriptsGroup> = files
        .iter()
        .filter(|rel| rel.file_name().and_then(|n| n.to_str()) == Some("package.json"))
        .filter_map(|rel| {
            let content = std::fs::read_to_string(dir.join(rel)).ok()?;
            let (package_name, scripts) = parse_package_json(&content)?;
            let parent = rel.parent().filter(|p| !p.as_os_str().is_empty());
            Some(PackageScriptsGroup {
                dir: parent.map(walk::to_slash).unwrap_or_else(|| ".".into()),
                package_name,
                scripts,
            })
        })
        .collect();
    // 根目录包优先,其余按目录字典序
    groups.sort_by(|a, b| (a.dir != ".", &a.dir).cmp(&(b.dir != ".", &b.dir)));
    groups
}

fn map_command_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CustomCommand> {
    Ok(CustomCommand {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        command: r.get(3)?,
        description: r.get(4)?,
        icon: r.get(5)?,
        sort_order: r.get(6)?,
    })
}

const COMMAND_COLS: &str = "id, project_id, name, command, description, icon, sort_order";

pub fn list_commands(conn: &Connection, project_id: i64) -> AppResult<Vec<CustomCommand>> {
    let sql = format!(
        "SELECT {COMMAND_COLS} FROM custom_commands WHERE project_id = ?1 ORDER BY sort_order, id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![project_id], map_command_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn get_command(conn: &Connection, id: i64) -> AppResult<CustomCommand> {
    let sql = format!("SELECT {COMMAND_COLS} FROM custom_commands WHERE id = ?1");
    conn.query_row(&sql, params![id], map_command_row)
        .optional()?
        .ok_or_else(|| AppError::coded(ErrorCode::CommandNotFound, id.to_string()))
}

fn validate_command(name: &str, command: &str) -> AppResult<()> {
    if name.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::CommandNameRequired, ""));
    }
    if command.trim().is_empty() {
        return Err(AppError::coded(ErrorCode::CommandContentRequired, ""));
    }
    Ok(())
}

fn to_conflict(e: rusqlite::Error, name: &str) -> AppError {
    match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            AppError::coded(ErrorCode::CommandNameConflict, name.to_string())
        }
        other => AppError::Db(other),
    }
}

pub fn create_command(
    conn: &Connection,
    project_id: i64,
    name: &str,
    command: &str,
    description: &str,
    icon: &str,
) -> AppResult<CustomCommand> {
    validate_command(name, command)?;
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1",
        params![project_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, project_id.to_string()));
    }
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM custom_commands WHERE project_id = ?1",
        params![project_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO custom_commands (project_id, name, command, description, icon, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project_id,
            name.trim(),
            command.trim(),
            description.trim(),
            icon.trim(),
            next_order
        ],
    )
    .map_err(|e| to_conflict(e, name))?;
    get_command(conn, conn.last_insert_rowid())
}

pub fn update_command(
    conn: &Connection,
    id: i64,
    name: &str,
    command: &str,
    description: &str,
    icon: &str,
) -> AppResult<CustomCommand> {
    validate_command(name, command)?;
    let changed = conn
        .execute(
            "UPDATE custom_commands SET name = ?1, command = ?2, description = ?3, icon = ?4 WHERE id = ?5",
            params![name.trim(), command.trim(), description.trim(), icon.trim(), id],
        )
        .map_err(|e| to_conflict(e, name))?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::CommandNotFound, id.to_string()));
    }
    // 同步可能存在的「常用命令」标记快照(target_key = 命令 id),避免编辑后快照失效
    conn.execute(
        "UPDATE pinned_commands SET label = ?1, command = ?2 WHERE kind = 'customCommand' AND target_key = ?3",
        params![name.trim(), command.trim(), id.to_string()],
    )?;
    get_command(conn, id)
}

pub fn delete_command(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute("DELETE FROM custom_commands WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::CommandNotFound, id.to_string()));
    }
    // 连带删除该命令的「常用命令」标记
    conn.execute(
        "DELETE FROM pinned_commands WHERE kind = 'customCommand' AND target_key = ?1",
        params![id.to_string()],
    )?;
    Ok(())
}

// ---- Tauri 命令包装 ----
// (列表查询已并入 commands::overview::get_project_overview,详情页一次 IPC 取全)

#[tauri::command]
pub fn create_custom_command(
    db: State<'_, Db>,
    project_id: i64,
    name: String,
    command: String,
    description: String,
    icon: String,
) -> AppResult<CustomCommand> {
    let conn = db.0.lock().unwrap();
    create_command(&conn, project_id, &name, &command, &description, &icon)
}

#[tauri::command]
pub fn update_custom_command(
    app: AppHandle,
    db: State<'_, Db>,
    id: i64,
    name: String,
    command: String,
    description: String,
    icon: String,
) -> AppResult<CustomCommand> {
    let result = {
        let conn = db.0.lock().unwrap();
        update_command(&conn, id, &name, &command, &description, &icon)?
    };
    // 编辑可能同步了「常用命令」标记快照,广播让另一窗口刷新
    let _ = app.emit("projects://pins-changed", serde_json::json!({}));
    Ok(result)
}

#[tauri::command]
pub fn delete_custom_command(app: AppHandle, db: State<'_, Db>, id: i64) -> AppResult<()> {
    {
        let conn = db.0.lock().unwrap();
        delete_command(&conn, id)?;
    }
    // 删除会连带移除「常用命令」标记,广播让另一窗口刷新
    let _ = app.emit("projects://pins-changed", serde_json::json!({}));
    Ok(())
}

/// 在系统终端执行命令(新窗口,跑完不关)。
/// cwd 指定工作目录(缺省用项目根 path),用于 monorepo 子包内执行 npm run。
/// java_home 非空时在命令前注入 JAVA_HOME(mvn.cmd 与 gradlew 均原生读取),
/// 用命令前缀而非进程环境注入:对 wt / cmd 兜底路径与 macOS Terminal 一致生效,
/// 且用户能在终端里直接看到当前生效的 JAVA_HOME。
#[tauri::command]
pub fn run_in_terminal(
    path: String,
    project_name: String,
    command: String,
    cwd: Option<String>,
    java_home: Option<String>,
) -> AppResult<()> {
    let work_dir = cwd.unwrap_or(path);
    if !std::path::Path::new(&work_dir).is_dir() {
        return Err(AppError::coded(ErrorCode::ScriptDirNotFound, work_dir));
    }
    let command = match java_home.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(home) => {
            let home = home.replace('"', "");
            if cfg!(windows) {
                format!("set \"JAVA_HOME={home}\" && {command}")
            } else {
                format!("export JAVA_HOME=\"{home}\" && {command}")
            }
        }
        None => command,
    };
    spawn_terminal(
        &work_dir,
        &format!("Project: {project_name}"),
        Some(&command),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::project;
    use crate::db;
    use std::fs;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    fn add_project(conn: &Connection) -> i64 {
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        project::add(conn, &dir, "demo", "").unwrap().id
    }

    fn temp_project_dir(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "repomeow-pkg-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn parses_package_scripts() {
        let dir = temp_project_dir("basic");
        let p = std::path::Path::new(&dir);

        assert!(package_scripts(&dir).unwrap().is_empty());

        fs::write(p.join("package.json"), "{ not json").unwrap();
        assert!(package_scripts(&dir).unwrap().is_empty());

        fs::write(
            p.join("package.json"),
            r#"{"name":"demo","scripts":{"dev":"vite","build":"vite build"}}"#,
        )
        .unwrap();
        let groups = package_scripts(&dir).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].dir, ".");
        assert_eq!(groups[0].package_name.as_deref(), Some("demo"));
        assert_eq!(groups[0].scripts.len(), 2);
        assert!(groups[0]
            .scripts
            .iter()
            .any(|s| s.name == "dev" && s.command == "vite"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovers_monorepo_packages() {
        let dir = temp_project_dir("monorepo");
        let p = std::path::Path::new(&dir);

        fs::write(
            p.join("package.json"),
            r#"{"name":"root","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        fs::create_dir_all(p.join("packages/api")).unwrap();
        fs::write(
            p.join("packages/api/package.json"),
            r#"{"name":"@app/api","scripts":{"start":"node index.js","test":"vitest"}}"#,
        )
        .unwrap();
        // 无 scripts 字段 -> 跳过
        fs::create_dir_all(p.join("packages/empty")).unwrap();
        fs::write(
            p.join("packages/empty/package.json"),
            r#"{"name":"@app/empty"}"#,
        )
        .unwrap();
        // scripts 为空对象 -> 跳过
        fs::create_dir_all(p.join("packages/none")).unwrap();
        fs::write(p.join("packages/none/package.json"), r#"{"scripts":{}}"#).unwrap();
        // node_modules 中的 package.json 恒跳过(即使未被 gitignore)
        fs::create_dir_all(p.join("node_modules/dep")).unwrap();
        fs::write(
            p.join("node_modules/dep/package.json"),
            r#"{"scripts":{"x":"y"}}"#,
        )
        .unwrap();

        let groups = package_scripts(&dir).unwrap();
        assert_eq!(groups.len(), 2);
        // 根目录包优先
        assert_eq!(groups[0].dir, ".");
        assert_eq!(groups[1].dir, "packages/api");
        assert_eq!(groups[1].package_name.as_deref(), Some("@app/api"));
        assert_eq!(groups[1].scripts.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn respects_gitignore_when_scanning_packages() {
        let dir = temp_project_dir("gitignore");
        let p = std::path::Path::new(&dir);

        fs::write(p.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        fs::create_dir_all(p.join("vendored/ui")).unwrap();
        fs::write(
            p.join("vendored/ui/package.json"),
            r#"{"scripts":{"build":"x"}}"#,
        )
        .unwrap();
        fs::write(p.join(".gitignore"), "vendored/\n").unwrap();

        let groups = package_scripts(&dir).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].dir, ".");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_command_crud() {
        let conn = test_conn();
        let pid = add_project(&conn);

        let c = create_command(&conn, pid, "clean", "rm -rf dist", "清理产物", "rocket").unwrap();
        assert_eq!(c.sort_order, 0);
        assert_eq!(c.icon, "rocket");
        let c2 = create_command(&conn, pid, "logs", "tail -f app.log", "", "").unwrap();
        assert_eq!(c2.sort_order, 1);

        let list = list_commands(&conn, pid).unwrap();
        assert_eq!(list.len(), 2);

        let updated =
            update_command(&conn, c.id, "clean2", "rm -rf build", "改后", "wrench").unwrap();
        assert_eq!(updated.name, "clean2");
        assert_eq!(updated.description, "改后");
        assert_eq!(updated.icon, "wrench");

        // 重名冲突
        assert!(matches!(
            update_command(&conn, c.id, "logs", "x", "", ""),
            Err(ref e) if e.is_code(ErrorCode::CommandNameConflict)
        ));

        delete_command(&conn, c2.id).unwrap();
        assert_eq!(list_commands(&conn, pid).unwrap().len(), 1);
        assert!(matches!(
            delete_command(&conn, c2.id),
            Err(ref e) if e.is_code(ErrorCode::CommandNotFound)
        ));
    }

    #[test]
    fn custom_command_validates_and_cascades() {
        let conn = test_conn();
        let pid = add_project(&conn);
        assert!(matches!(
            create_command(&conn, pid, "", "x", "", ""),
            Err(ref e) if e.is_code(ErrorCode::CommandNameRequired)
        ));
        assert!(matches!(
            create_command(&conn, 9999, "a", "b", "", ""),
            Err(ref e) if e.is_code(ErrorCode::ProjectNotFound)
        ));

        create_command(&conn, pid, "a", "b", "", "").unwrap();
        // 项目已无删除入口(仅归档),这里用原生 SQL 验证 schema 的外键级联仍然有效
        conn.execute("DELETE FROM projects WHERE id = ?1", [pid])
            .unwrap();
        assert!(list_commands(&conn, pid).unwrap().is_empty());
    }
}
