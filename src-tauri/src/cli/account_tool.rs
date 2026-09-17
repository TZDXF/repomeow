//! CLI `account` 分组:Git 平台账号(github/gitee/gitlab)管理。

use std::path::Path;

use serde_json::{json, Value};

use crate::commands::account;

use super::util::{data_root_or_default, open_db, ToolFailure};

pub(super) fn list_accounts_impl(data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    let accounts = account::list_accounts(&conn)
        .map_err(|error| ToolFailure::from_app("查询 Git 账号失败", error))?;
    Ok(json!({ "accounts": accounts }))
}

/// 绑定账号:先调平台 API 验证 token,成功才落库。
pub(super) async fn add_account_impl(
    provider: &str,
    label: &str,
    base_url: Option<&str>,
    token: &str,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let account = account::add_account(&db, provider, label, base_url, token)
        .await
        .map_err(|error| ToolFailure::from_app("添加 Git 账号失败", error))?;
    Ok(json!(account))
}

/// 更新账号;--token 缺省表示保留原 token,token 或实例地址变化时重新验证。
pub(super) async fn update_account_impl(
    id: i64,
    label: &str,
    base_url: Option<&str>,
    token: Option<&str>,
    data_root: Option<&Path>,
) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let account = account::update_account(&db, id, label, base_url, token)
        .await
        .map_err(|error| ToolFailure::from_app("更新 Git 账号失败", error))?;
    Ok(json!(account))
}

pub(super) fn remove_account_impl(id: i64, data_root: Option<&Path>) -> Result<Value, ToolFailure> {
    let data_root = data_root_or_default(data_root)?;
    let db = open_db(&data_root)?;
    let conn = db.0.lock().unwrap();
    account::remove_account(&conn, id)
        .map_err(|error| ToolFailure::from_app("删除 Git 账号失败", error))?;
    Ok(json!({ "id": id, "deleted": true }))
}
