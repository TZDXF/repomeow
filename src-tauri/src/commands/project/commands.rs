use tauri::{AppHandle, Emitter, State};

use super::move_dir::{apply_move, move_folder, prepare_move};
use super::repository::{
    add, archive, get, list, list_archived, remove, set_auto_pull, set_favorite,
    set_wiki_auto_update, unarchive, update, update_path,
};
use crate::db::Db;
use crate::error::AppResult;
use crate::models::Project;

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
    get(&conn, id)
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
pub fn set_project_wiki_auto_update(db: State<'_, Db>, id: i64, enabled: bool) -> AppResult<()> {
    let conn = db.0.lock().unwrap();
    set_wiki_auto_update(&conn, id, enabled)
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
