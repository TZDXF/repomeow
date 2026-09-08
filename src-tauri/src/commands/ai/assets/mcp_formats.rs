//! codex `.codex/config.toml` 的 MCP 服务器读写。
//!
//! codex 的 MCP 配置是 TOML 表(`[mcp_servers.<name>]`:command/args/env,
//! 远程为 url 等)。写入经 toml_edit 做**保格式编辑**——手写注释与排版不丢;
//! 读侧把 TOML 表转成与 JSON 方言一致的 `{ name, config }` 结构。

use std::fs;
use std::path::Path;

use serde_json::Value;
use toml_edit::{DocumentMut, Item, Table};

use crate::error::{AppError, AppResult, ErrorCode};

use super::McpServerEntry;

/// 读 mcp_servers 表为 JSON 条目(按名称排序);文件缺失或解析失败返回空。
pub(super) fn read_codex_servers(path: &Path) -> Vec<McpServerEntry> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = raw.parse::<DocumentMut>() else {
        return Vec::new();
    };
    let Some(servers) = doc.get("mcp_servers").and_then(Item::as_table) else {
        return Vec::new();
    };
    let mut entries: Vec<McpServerEntry> = servers
        .iter()
        .filter_map(|(name, item)| {
            Some(McpServerEntry {
                name: name.to_string(),
                config: toml_item_to_json(item)?,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// JSON 服务器定义 → TOML 条目。对象转普通表,嵌套对象(env 等)渲染为
/// `[mcp_servers.<name>.env]` 子表;JSON null 在 TOML 无对应,直接报错。
pub(super) fn json_to_toml_item(value: &Value) -> AppResult<Item> {
    if let Value::Object(map) = value {
        let mut table = Table::new();
        for (key, entry) in map {
            table.insert(key, json_to_toml_item(entry)?);
        }
        return Ok(Item::Table(table));
    }
    Ok(Item::Value(json_to_toml_value(value)?))
}

/// JSON 标量/数组/对象 → TOML 值;数组里的对象转内联表(TOML 数组元素
/// 不能是普通子表)。
fn json_to_toml_value(value: &Value) -> AppResult<toml_edit::Value> {
    match value {
        Value::Bool(b) => Ok((*b).into()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into())
            } else {
                let f = n.as_f64().ok_or_else(|| {
                    AppError::coded(ErrorCode::InvalidPath, "不支持的数值".to_string())
                })?;
                Ok(f.into())
            }
        }
        Value::String(s) => Ok(s.as_str().into()),
        Value::Array(items) => {
            let mut arr = toml_edit::Array::new();
            for item in items {
                arr.push(json_to_toml_value(item)?);
            }
            Ok(arr.into())
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, entry) in map {
                table.insert(key, json_to_toml_value(entry)?);
            }
            Ok(table.into())
        }
        Value::Null => Err(AppError::coded(
            ErrorCode::InvalidPath,
            "服务器定义含 null,TOML 不支持".to_string(),
        )),
    }
}

fn toml_item_to_json(item: &Item) -> Option<Value> {
    match item {
        Item::Value(value) => toml_value_to_json(value),
        Item::Table(table) => {
            let mut map = serde_json::Map::new();
            for (key, entry) in table.iter() {
                map.insert(key.to_string(), toml_item_to_json(entry)?);
            }
            Some(Value::Object(map))
        }
        Item::ArrayOfTables(tables) => {
            let mut list = Vec::new();
            for table in tables.iter() {
                list.push(toml_item_to_json(&Item::Table(table.clone()))?);
            }
            Some(Value::Array(list))
        }
        Item::None => None,
    }
}

fn toml_value_to_json(value: &toml_edit::Value) -> Option<Value> {
    match value {
        toml_edit::Value::String(s) => Some(Value::String(s.value().to_string())),
        toml_edit::Value::Integer(i) => Some(Value::Number((*i.value()).into())),
        toml_edit::Value::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number),
        toml_edit::Value::Boolean(b) => Some(Value::Bool(*b.value())),
        toml_edit::Value::Datetime(d) => Some(Value::String(d.to_string())),
        toml_edit::Value::Array(arr) => {
            let mut list = Vec::new();
            for entry in arr.iter() {
                list.push(toml_value_to_json(entry)?);
            }
            Some(Value::Array(list))
        }
        toml_edit::Value::InlineTable(table) => {
            let mut map = serde_json::Map::new();
            for (key, entry) in table.iter() {
                map.insert(key.to_string(), toml_value_to_json(entry)?);
            }
            Some(Value::Object(map))
        }
    }
}
