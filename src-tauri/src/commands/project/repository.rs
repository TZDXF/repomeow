use std::collections::HashMap;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};

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
    wiki_auto_update: bool,
    created_at: i64,
    updated_at: i64,
}

const PROJECT_COLS: &str = "id, path, name, description, archived_at, favorited_at, auto_pull, wiki_auto_update, created_at, updated_at";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
    Ok(ProjectRow {
        id: r.get(0)?,
        path: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        archived_at: r.get(4)?,
        favorited_at: r.get(5)?,
        auto_pull: r.get(6)?,
        wiki_auto_update: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
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
        wiki_auto_update: row.wiki_auto_update,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn load_tags_by_project(
    conn: &Connection,
    project_ids: &[i64],
) -> AppResult<HashMap<i64, Vec<Tag>>> {
    let mut tags_by_project = HashMap::new();
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
    ensure_project_changed(changed, id)
}

/// 设置/取消「跟踪更新」:开启后后台循环在远端有更新时自动快进拉取
/// (无法快进即取消,不提醒)。归档项目不参与后台循环,但开关状态保留
pub fn set_auto_pull(conn: &Connection, id: i64, enabled: bool) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE projects SET auto_pull = ?1 WHERE id = ?2",
        params![enabled, id],
    )?;
    ensure_project_changed(changed, id)
}

/// 设置/取消项目级「Wiki 自动增量更新」:本地 HEAD 变化后独立触发,
/// 与「跟踪更新」(auto_pull)互不依赖
pub fn set_wiki_auto_update(conn: &Connection, id: i64, enabled: bool) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE projects SET wiki_auto_update = ?1 WHERE id = ?2",
        params![enabled, id],
    )?;
    ensure_project_changed(changed, id)
}

fn ensure_project_changed(changed: usize, id: i64) -> AppResult<()> {
    if changed == 0 {
        return Err(AppError::coded(ErrorCode::ProjectNotFound, id.to_string()));
    }
    Ok(())
}

/// 彻底删除项目(关联的标签指派、自定义命令随外键级联清理;不动磁盘文件)
pub fn remove(conn: &Connection, id: i64) -> AppResult<()> {
    let changed = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    ensure_project_changed(changed, id)
}

/// 启动时清洗存量登记路径:统一分隔符、去尾随分隔符(入库归一化前的历史数据)。
/// 当前版本未发布,直接原地改写;清洗后与其他行冲突(历史重复登记)时跳过该行并记日志
pub fn normalize_stored_paths(conn: &Connection) -> usize {
    let rows: Vec<(i64, String)> = conn
        .prepare("SELECT id, path FROM projects")
        .and_then(|mut stmt| {
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
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
