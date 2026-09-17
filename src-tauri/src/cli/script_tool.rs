//! CLI `script` 分组:项目自定义命令管理(复用 commands::script 的纯函数)。

use std::path::Path;

use serde_json::{json, Value};

use crate::commands::script;

use super::util::{data_root_or_default, open_db, require_project_id, ToolFailure};

pub(super) fn create_command_impl(
    project_directory: &str,
    name: &str,
    command: &str,
    description: Option<&str>,
    icon: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, project_directory)?;
    let created = script::create_command(
        &conn,
        project_id,
        name,
        command,
        description.unwrap_or(""),
        icon.unwrap_or(""),
    )
    .map_err(|error| ToolFailure::from_app("创建自定义命令失败", error))?;
    Ok(json!(created))
}

pub(super) fn update_command_impl(
    id: i64,
    name: &str,
    command: &str,
    description: Option<&str>,
    icon: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    // 与设置页编辑行为一致:全量提交 name/command,description/icon 缺省置空
    let updated = script::update_command(
        &conn,
        id,
        name,
        command,
        description.unwrap_or(""),
        icon.unwrap_or(""),
    )
    .map_err(|error| ToolFailure::from_app("更新自定义命令失败", error))?;
    Ok(json!(updated))
}

pub(super) fn delete_command_impl(id: i64, data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    script::delete_command(&conn, id)
        .map_err(|error| ToolFailure::from_app("删除自定义命令失败", error))?;
    Ok(json!({ "id": id, "deleted": true }))
}
