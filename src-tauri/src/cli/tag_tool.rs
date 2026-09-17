//! CLI `tag` 分组:标签 CRUD 与项目标签绑定(纯 SQLite,复用 commands::tag)。

use std::path::Path;

use serde_json::{json, Value};

use crate::commands::tag;

use super::util::{data_root_or_default, open_db, require_project_id, ToolFailure};

pub(super) fn list_tags_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let tags = tag::all(&conn).map_err(|error| ToolFailure::from_app("查询标签失败", error))?;
    Ok(json!({ "tags": tags }))
}

pub(super) fn create_tag_impl(
    name: &str,
    color: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let created = tag::create(&conn, name, color.unwrap_or(""))
        .map_err(|error| ToolFailure::from_app("创建标签失败", error))?;
    Ok(json!(created))
}

pub(super) fn update_tag_impl(
    id: i64,
    name: &str,
    color: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let updated = tag::update(&conn, id, name, color.unwrap_or(""))
        .map_err(|error| ToolFailure::from_app("更新标签失败", error))?;
    Ok(json!(updated))
}

pub(super) fn delete_tag_impl(id: i64, data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    tag::remove(&conn, id).map_err(|error| ToolFailure::from_app("删除标签失败", error))?;
    Ok(json!({ "id": id, "deleted": true }))
}

/// 全量覆盖项目的标签绑定(传空列表即清空)。
pub(super) fn set_project_tags_impl(
    project_directory: &str,
    tag_ids: Vec<i64>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, project_directory)?;
    tag::apply_project_tags(&conn, project_id, &tag_ids)
        .map_err(|error| ToolFailure::from_app("设置项目标签失败", error))?;
    Ok(json!({ "projectId": project_id, "tagIds": tag_ids }))
}
