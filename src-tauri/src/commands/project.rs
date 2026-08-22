use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
use tauri::{AppHandle, Emitter, State};

use crate::db::Db;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models::{Project, Tag};
use crate::time_util::now_ts;

struct ProjectRow {
    id: i64,
    path: String,
    name: String,
    description: String,
    archived_at: Option<i64>,
    favorited_at: Option<i64>,
    auto_pull: bool,
    created_at: i64,
    updated_at: i64,
}

const PROJECT_COLS: &str =
    "id, path, name, description, archived_at, favorited_at, auto_pull, created_at, updated_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: r.get(0)?,
        path: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        archived_at: r.get(4)?,
        favorited_at: r.get(5)?,
        auto_pull: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

pub fn load_tags(conn: &Connection, project_id: i64) -> AppResult<Vec<Tag>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.color
         FROM tags t
         JOIN project_tags pt ON pt.tag_id = t.id
         WHERE pt.project_id = ?1
         ORDER BY t.name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![project_id], |r| {
        Ok(Tag {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn project_from_row(row: ProjectRow, tags: Vec<Tag>) -> Project {
    let path_exists = std::path::Path::new(&row.path).is_dir();
    Project {
        id: row.id,
        path: row.path,
        name: row.name,
        description: row.description,
        tags,
        git: None,
        path_exists,
        archived_at: row.archived_at,
        favorited_at: row.favorited_at,
        auto_pull: row.auto_pull,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn load_tags_by_project(
    conn: &Connection,
    project_ids: &[i64],
) -> AppResult<std::collections::HashMap<i64, Vec<Tag>>> {
    let mut tags_by_project = std::collections::HashMap::new();
    if project_ids.is_empty() {
        return Ok(tags_by_project);
    }

    let placeholders = vec!["?"; project_ids.len()].join(",");
    let sql = format!(
        "SELECT pt.project_id, t.id, t.name, t.color
         FROM project_tags pt
         JOIN tags t ON pt.tag_id = t.id
         WHERE pt.project_id IN ({placeholders})
         ORDER BY pt.project_id, t.name COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(project_ids.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            Tag {
                id: r.get(1)?,
                name: r.get(2)?,
                color: r.get(3)?,
            },
        ))
    })?;
    for row in rows {
        let (project_id, tag) = row?;
        tags_by_project
            .entry(project_id)
            .or_insert_with(Vec::new)
            .push(tag);
    }
    Ok(tags_by_project)
}

fn with_tags(conn: &Connection, row: ProjectRow) -> AppResult<Project> {
    let tags = load_tags(conn, row.id)?;
    Ok(project_from_row(row, tags))
}

fn projects_with_tags(conn: &Connection, rows: Vec<ProjectRow>) -> AppResult<Vec<Project>> {
    let project_ids: Vec<_> = rows.iter().map(|row| row.id).collect();
    let mut tags_by_project = load_tags_by_project(conn, &project_ids)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let project_id = row.id;
            project_from_row(row, tags_by_project.remove(&project_id).unwrap_or_default())
        })
        .collect())
}

pub fn add(conn: &Connection, path: &str, name: &str, description: &str) -> AppResult<Project> {
    // 入库前统一路径形态:同一目录的正反斜杠/尾斜杠写法归一,
    // 否则 SQLite UNIQUE 是字面比较,`C:\repo` 与 `C:/repo` 会登记成两个项目
    let path = crate::path_util::clean_str(path);
    if !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, &path));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::coded(ErrorCode::ProjectNameRequired, ""));
    }
    let ts = now_ts();
    conn.execute(
        "INSERT INTO projects (path, name, description, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![path, name, description.trim(), ts, ts],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            AppError::coded(ErrorCode::ProjectPathConflict, &path)
        }
        other => AppError::Db(other),
    })?;
    get(conn, conn.last_insert_rowid())
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Project> {
    let sql = format!("SELECT {PROJECT_COLS} FROM projects WHERE id = ?1");
    let row = conn.query_row(&sql, params![id], map_row).optional()?;
    match row {
        Some(r) => with_tags(conn, r),
        None => Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string())),
    }
}

pub fn list(
    conn: &Connection,
    query: Option<String>,
    tag_ids: Option<Vec<i64>>,
) -> AppResult<Vec<Project>> {
    let mut sql = format!("SELECT {PROJECT_COLS} FROM projects");
    // 归档项目不出现在列表中(数据保留,但不展示、不获取 git 状态)
    let mut conditions: Vec<String> = vec!["archived_at IS NULL".to_string()];
    let mut binds: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
        // 空格切分为多个查询词,词间 AND:每个词命中名称或描述之一
        for term in q.split_whitespace() {
            conditions.push("(name LIKE ? OR description LIKE ?)".to_string());
            let pattern = format!("%{}%", term);
            binds.push(Box::new(pattern.clone()));
            binds.push(Box::new(pattern));
        }
    }
    if let Some(ids) = tag_ids.filter(|v| !v.is_empty()) {
        let placeholders = vec!["?"; ids.len()].join(",");
        conditions.push(format!(
            "id IN (SELECT project_id FROM project_tags WHERE tag_id IN ({placeholders}))"
        ));
        for id in ids {
            binds.push(Box::new(id));
        }
    }
    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(" ORDER BY name COLLATE NOCASE");

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds.iter()), map_row)?;
    let project_rows = rows.collect::<Result<Vec<_>, _>>()?;
    projects_with_tags(conn, project_rows)
}

pub fn update(conn: &Connection, id: i64, name: &str, description: &str) -> AppResult<Project> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::coded(ErrorCode::ProjectNameRequired, ""));
    }
    let changed = conn.execute(
        "UPDATE projects SET name = ?1, description = ?2, updated_at = ?3 WHERE id = ?4",
        params![name, description, now_ts(), id],
    )?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    get(conn, id)
}

/// 重新指定项目目录（项目被移动后修复登记路径；标签、自定义命令等关联随 id 保留）
pub fn update_path(conn: &Connection, id: i64, path: &str) -> AppResult<Project> {
    let path = crate::path_util::clean_str(path);
    if path.is_empty() || !std::path::Path::new(&path).is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, &path));
    }
    let changed = conn
        .execute(
            "UPDATE projects SET path = ?1, updated_at = ?2 WHERE id = ?3",
            params![path, now_ts(), id],
        )
        .map_err(|e| match e {
            rusqlite::Error::SqliteFailure(err, _)
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                AppError::coded(ErrorCode::ProjectPathConflict, &path)
            }
            other => AppError::Db(other),
        })?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    get(conn, id)
}

/// 一次目录移动的校验结果(源/目标路径)
struct MovePlan {
    src: std::path::PathBuf,
    target: std::path::PathBuf,
    target_str: String,
}

/// 校验移动参数并计算目标路径(不触碰磁盘)
fn prepare_move(conn: &Connection, id: i64, target_parent: &str, dir_name: &str) -> AppResult<MovePlan> {
    let project = get(conn, id)?;
    let src = std::path::PathBuf::from(&project.path);
    if !src.is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, &project.path));
    }
    let parent = std::path::Path::new(target_parent.trim());
    if !parent.is_dir() {
        return Err(AppError::coded(ErrorCode::InvalidPath, target_parent.trim()));
    }
    let dir_name = dir_name.trim();
    if dir_name.is_empty() || dir_name == "." || dir_name == ".." || dir_name.contains('/')
        || dir_name.contains('\\')
    {
        return Err(AppError::coded(ErrorCode::MoveInvalidDirName, dir_name));
    }
    let target = parent.join(dir_name);
    // 目标路径归一化后再比较与落库:用户输入的 parent 可能是正斜杠风格,
    // 与库里登记的反斜杠路径字面不等会让"移动到自身位置"绕过 MoveSameLocation 检查
    let target_str = crate::path_util::clean(&target).to_string_lossy().to_string();
    // Windows 文件系统大小写不敏感,统一按忽略大小写判断"位置未变化"
    if target_str.eq_ignore_ascii_case(&project.path) {
        return Err(AppError::coded(ErrorCode::MoveSameLocation, ""));
    }
    if target.starts_with(&src) {
        return Err(AppError::coded(ErrorCode::MoveInsideSelf, ""));
    }
    if target.exists() {
        return Err(AppError::coded(
            ErrorCode::MoveTargetExists,
            target.to_string_lossy().to_string(),
        ));
    }
    // 目标路径已被其他项目登记时提前报错,避免移动后数据库唯一键冲突
    let registered = conn
        .query_row(
            "SELECT id FROM projects WHERE path = ?1 AND id != ?2",
            params![target_str, id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    if registered.is_some() {
        return Err(AppError::coded(ErrorCode::ProjectPathConflict, target_str));
    }
    Ok(MovePlan { src, target, target_str })
}

/// 磁盘移动:同盘直接 rename;跨盘退回"复制 + 删除源"
fn move_folder(src: &std::path::Path, target: &std::path::Path) -> AppResult<()> {
    match std::fs::rename(src, target) {
        Ok(()) => Ok(()),
        // Windows ERROR_NOT_SAME_DEVICE(17) / Unix EXDEV(18)
        Err(e) if matches!(e.raw_os_error(), Some(17) | Some(18)) => {
            copy_across_devices(src, target)
        }
        Err(e) => Err(AppError::Io(e)),
    }
}

/// 跨盘移动(Windows):robocopy 复制(保留 junction/symlink 结构,/SL)成功后删除源。
/// 不用 /MOVE:复制失败时源目录保持完整;目标半成品尽力清理。
#[cfg(windows)]
fn copy_across_devices(src: &std::path::Path, target: &std::path::Path) -> AppResult<()> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("robocopy")
        .arg(src)
        .arg(target)
        .args([
            "/E", "/SL", "/COPY:DAT", "/DCOPY:T", "/R:1", "/W:1", "/NFL", "/NDL", "/NJH", "/NJS",
            "/NP",
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(AppError::Io)?;
    // robocopy 退出码是位标记,< 8 均表示成功(0=无变化 1=已复制 2/4=额外/不匹配文件)
    let code = output.status.code().unwrap_or(16);
    if code >= 8 {
        let _ = std::fs::remove_dir_all(target);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tail: String = stdout.chars().rev().take(200).collect::<String>().chars().rev().collect();
        return Err(AppError::coded(
            ErrorCode::MoveRobocopyFailed,
            format!("code={code} tail={tail}"),
        ));
    }
    std::fs::remove_dir_all(src).map_err(AppError::Io)
}

/// 跨盘移动(非 Windows):递归复制(符号链接按链接重建)成功后删除源
#[cfg(not(windows))]
fn copy_across_devices(src: &std::path::Path, target: &std::path::Path) -> AppResult<()> {
    if let Err(e) = copy_dir_recursive(src, target) {
        let _ = std::fs::remove_dir_all(target);
        return Err(AppError::Io(e));
    }
    std::fs::remove_dir_all(src).map_err(AppError::Io)
}

#[cfg(not(windows))]
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            std::os::unix::fs::symlink(&link, &to)?;
        } else if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// 落库:更新登记路径;失败时尽力把文件夹移回原位
fn apply_move(conn: &Connection, id: i64, plan: &MovePlan) -> AppResult<()> {
    if let Err(e) = conn.execute(
        "UPDATE projects SET path = ?1, updated_at = ?2 WHERE id = ?3",
        params![plan.target_str, now_ts(), id],
    ) {
        let _ = std::fs::rename(&plan.target, &plan.src);
        return Err(AppError::Db(e));
    }
    Ok(())
}

/// 应用内移动项目目录:把项目文件夹移动到新的父目录下(可同时改名),并更新登记路径。
/// 同盘直接 rename;跨盘自动退回"复制 + 删除源"(大目录耗时较长,由异步命令承载)。
// 命令端为不持锁移动拆成了 prepare_move/move_folder/apply_move 三阶段,此组合函数供测试使用
#[allow(dead_code)]
pub fn move_dir(conn: &Connection, id: i64, target_parent: &str, dir_name: &str) -> AppResult<Project> {
    let plan = prepare_move(conn, id, target_parent, dir_name)?;
    move_folder(&plan.src, &plan.target)?;
    apply_move(conn, id, &plan)?;
    get(conn, id)
}

/// 归档项目:软删除,保留历史数据(标签、自定义命令等关联数据不动)
pub fn archive(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE projects SET archived_at = ?1 WHERE id = ?2",
        params![now_ts(), id],
    )?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    Ok(())
}

/// 列出已归档项目(按归档时间倒序,设置页归档管理用)
pub fn list_archived(conn: &Connection) -> AppResult<Vec<Project>> {
    let sql = format!(
        "SELECT {PROJECT_COLS} FROM projects WHERE archived_at IS NOT NULL ORDER BY archived_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], map_row)?;
    let project_rows = rows.collect::<Result<Vec<_>, _>>()?;
    projects_with_tags(conn, project_rows)
}

/// 取消归档:恢复到项目列表
pub fn unarchive(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE projects SET archived_at = NULL WHERE id = ?1 AND archived_at IS NOT NULL",
        params![id],
    )?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    Ok(())
}

/// 设置/取消收藏:收藏项目在各列表中置顶(组内按收藏时间倒序)
pub fn set_favorite(conn: &Connection, id: i64, favorite: bool) -> AppResult<()> {
    let favorited_at = if favorite { Some(now_ts()) } else { None };
    let changed = conn.execute(
        "UPDATE projects SET favorited_at = ?1 WHERE id = ?2",
        params![favorited_at, id],
    )?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    Ok(())
}

/// 设置/取消「跟踪更新」:开启后后台循环在远端有更新时自动快进拉取
/// (无法快进即取消,不提醒)。归档项目不参与后台循环,但开关状态保留
pub fn set_auto_pull(conn: &Connection, id: i64, enabled: bool) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE projects SET auto_pull = ?1 WHERE id = ?2",
        params![enabled, id],
    )?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    Ok(())
}

/// 彻底删除项目(关联的标签指派、自定义命令随外键级联清理;不动磁盘文件)
pub fn remove(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    Ok(())
}

/// 启动时清洗存量登记路径:统一分隔符、去尾随分隔符(入库归一化前的历史数据)。
/// 当前版本未发布,直接原地改写;清洗后与其他行冲突(历史重复登记)时跳过该行并记日志
pub fn normalize_stored_paths(conn: &Connection) -> usize {
    let rows: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM projects")
        .and_then(|mut stmt| stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .unwrap_or_default();
    let mut changed = 0;
    for (id, path) in rows {
        let cleaned = crate::path_util::clean_str(&path);
        if cleaned == path {
            continue;
        }
        match conn.execute(
            "UPDATE projects SET path = ?1 WHERE id = ?2",
            params![cleaned, id],
        ) {
            Ok(_) => changed += 1,
            Err(e) => eprintln!("[project] 路径清洗跳过(id={id} path={path}): {e}"),
        }
    }
    if changed > 0 {
        eprintln!("[project] 已清洗 {changed} 条存量登记路径的分隔符风格");
    }
    changed
}

// ---- Tauri 命令包装 ----

#[tauri::command]
pub fn add_project(
    db: State<'_, Db>,
    path: String,
    name: String,
    description: Option<String>,
) -> AppResult<Project> {
    let conn = db.0.lock().unwrap();
    add(&conn, &path, &name, description.as_deref().unwrap_or(""))
}

#[tauri::command]
pub fn list_projects(
    db: State<'_, Db>,
    query: Option<String>,
    tag_ids: Option<Vec<i64>>,
) -> AppResult<Vec<Project>> {
    let conn = db.0.lock().unwrap();
    list(&conn, query, tag_ids)
}

#[tauri::command]
pub fn get_project(db: State<'_, Db>, id: i64) -> AppResult<Project> {
    let conn = db.0.lock().unwrap();
    let project = get(&conn, id)?;
    Ok(project)
}

#[tauri::command]
pub fn update_project(
    db: State<'_, Db>,
    id: i64,
    name: String,
    description: String,
) -> AppResult<Project> {
    let conn = db.0.lock().unwrap();
    update(&conn, id, &name, &description)
}

#[tauri::command]
pub fn update_project_path(db: State<'_, Db>, id: i64, path: String) -> AppResult<Project> {
    let conn = db.0.lock().unwrap();
    let old_path = get(&conn, id)?.path;
    let project = update_path(&conn, id, &path)?;
    if old_path != crate::path_util::clean_str(&path) {
        // 旧路径不再指向该项目,清理其 git 状态与 walk 缓存/文件监听
        // (新路径由前端主动刷新回填,walk 监听在下次扫描时按需安装)
        crate::commands::git::invalidate_status(&old_path);
        crate::commands::walk::invalidate(std::path::Path::new(&old_path));
    }
    Ok(project)
}

// 异步命令:跨盘移动退回复制后大目录耗时较长,避免阻塞主线程。
// 校验与落库仍持锁快速完成,磁盘移动阶段不持有数据库锁。
#[tauri::command]
pub async fn move_project_dir(
    db: State<'_, Db>,
    id: i64,
    target_parent: String,
    dir_name: String,
) -> AppResult<Project> {
    let plan = {
        let conn = db.0.lock().unwrap();
        prepare_move(&conn, id, &target_parent, &dir_name)?
    };
    move_folder(&plan.src, &plan.target)?;
    // 磁盘移动成功后,旧路径的 git 状态与 walk 缓存/文件监听已失效
    crate::commands::git::invalidate_status(&plan.src.to_string_lossy());
    crate::commands::walk::invalidate(&plan.src);
    let conn = db.0.lock().unwrap();
    apply_move(&conn, id, &plan)?;
    get(&conn, id)
}

#[tauri::command]
pub fn archive_project(db: State<'_, Db>, id: i64) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    archive(&conn, id)
}

#[tauri::command]
pub fn list_archived_projects(db: State<'_, Db>) -> AppResult<Vec<Project>> {
    let conn = db.0.lock().unwrap();
    list_archived(&conn)
}

#[tauri::command]
pub fn unarchive_project(db: State<'_, Db>, id: i64) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    unarchive(&conn, id)
}

#[tauri::command]
pub fn set_project_favorite(
    app: AppHandle,
    db: State<'_, Db>,
    id: i64,
    favorite: bool,
) -> AppResult<()> {
    {
        let conn = db.0.lock().unwrap();
        set_favorite(&conn, id, favorite)?;
    }
    // 托盘弹窗与主窗口是独立 Pinia 实例,广播收藏变更让另一窗口同步刷新
    let _ = app.emit(
        "projects://favorite-changed",
        serde_json::json!({ "id": id, "favorite": favorite }),
    );
    Ok(())
}

#[tauri::command]
pub fn set_project_auto_pull(db: State<'_, Db>, id: i64, enabled: bool) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    set_auto_pull(&conn, id, enabled)
}

#[tauri::command]
pub fn delete_project(db: State<'_, Db>, id: i64) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    let project = get(&conn, id)?;
    remove(&conn, id)?;
    crate::commands::git::invalidate_status(&project.path);
    crate::commands::walk::invalidate(std::path::Path::new(&project.path));
    Ok(())
}

#[cfg(test)]
mod tests {
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
        assert!(
            list(&conn, Some("web  alpha   beta ".into()), None)
                .unwrap()
                .is_empty()
        );
    }
}
