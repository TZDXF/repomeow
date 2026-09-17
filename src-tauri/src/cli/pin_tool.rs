//! CLI `pin` / `hidden` 分组:常用命令标记与条目隐藏(复用 commands::pin / hidden)。

use std::path::Path;

use serde_json::{json, Value};

use crate::commands::{hidden, pin};

use super::util::{data_root_or_default, open_db, require_project_id, ToolFailure};

pub(super) fn list_pins_impl(
    project_directory: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = match project_directory {
        Some(directory) => Some(require_project_id(&conn, directory)?),
        None => None,
    };
    let pins = pin::list(&conn, project_id)
        .map_err(|error| ToolFailure::from_app("查询常用命令失败", error))?;
    Ok(json!({ "pins": pins }))
}

/// 标记/取消标记常用命令。kind 取值见应用:packageScript/composeFile/composeService/customCommand/javaBuild。
#[allow(clippy::too_many_arguments)]
pub(super) fn set_pin_impl(
    project_directory: &str,
    kind: &str,
    target_key: &str,
    pinned: bool,
    label: Option<&str>,
    command: Option<&str>,
    cwd: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, project_directory)?;
    pin::set_pinned(
        &conn,
        project_id,
        kind,
        target_key,
        pinned,
        label.unwrap_or(""),
        command.unwrap_or(""),
        cwd,
    )
    .map_err(|error| ToolFailure::from_app("设置常用命令标记失败", error))?;
    Ok(json!({
        "projectId": project_id,
        "kind": kind,
        "targetKey": target_key,
        "pinned": pinned,
    }))
}

pub(super) fn list_hidden_impl(
    project_directory: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, project_directory)?;
    let items = hidden::list(&conn, project_id)
        .map_err(|error| ToolFailure::from_app("查询隐藏项失败", error))?;
    Ok(json!({ "projectId": project_id, "hidden": items }))
}

/// 隐藏/取消隐藏条目。kind 取值:packageFile/packageScript/composeFile/javaBuild。
pub(super) fn set_hidden_impl(
    project_directory: &str,
    kind: &str,
    target_key: &str,
    hidden_flag: bool,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let project_id = require_project_id(&conn, project_directory)?;
    hidden::set_hidden(&conn, project_id, kind, target_key, hidden_flag)
        .map_err(|error| ToolFailure::from_app("设置隐藏项失败", error))?;
    Ok(json!({
        "projectId": project_id,
        "kind": kind,
        "targetKey": target_key,
        "hidden": hidden_flag,
    }))
}
